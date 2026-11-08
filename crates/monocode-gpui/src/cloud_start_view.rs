//! What the UI knows about a cloud session whose codespace (or Docker sandbox) is starting.
//! The shell owns the state and publishes an immutable [`CloudStartSnapshot`] to the
//! conversation (composer chip, transcript status row) and to the Codespace panel.
use monocode_core::cloud_sessions::{CloudBackendKind, StartRecord, Step, StepKind, StepStatus};
use std::sync::Arc;

/// Log lines kept in memory per start.
pub(crate) const LOG_CAP: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartPhase {
    Starting,
    Failed,
    Ready,
}

/// What a finished start looked like, kept in memory so the UI can say "Codespace ready in X".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadySummary {
    pub total_ms: u64,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug)]
pub(crate) struct CloudStartSnapshot {
    /// The latest record. After `Ready` it is the last one seen (the store dropped it).
    pub record: StartRecord,
    pub phase: StartPhase,
    pub log: Arc<Vec<String>>,
    /// Read by the transcript status row ("Codespace ready in X").
    pub ready: Option<ReadySummary>,
    /// A worker is running `run_start` for this session.
    #[allow(dead_code)]
    pub running: bool,
}

impl CloudStartSnapshot {
    /// The backend the start runs on, which words every label below.
    pub(crate) fn backend(&self) -> CloudBackendKind {
        self.record.backend()
    }
    /// Lowercase word for sentences: `codespace` or `sandbox`.
    pub(crate) fn noun(&self) -> &'static str {
        self.backend().noun()
    }
    /// Capitalised [`Self::noun`]: the panel tab and the headlines.
    pub(crate) fn title(&self) -> &'static str {
        self.backend().title()
    }
    /// Whether the workspace can be opened in a browser. A Docker sandbox has no web view
    /// (`DockerBackend::web_url` is `None`), so the button is not offered.
    pub(crate) fn offers_browser(&self) -> bool {
        self.backend() == CloudBackendKind::Codespaces
    }
    /// The start is not over: sends are queued and the host is not loaded.
    pub(crate) fn blocks_session(&self) -> bool {
        self.phase != StartPhase::Ready
    }
    pub(crate) fn error(&self) -> Option<&str> {
        self.record.error.as_deref()
    }

    /// The step the failure happened in, for "Codespace failed at X".
    fn failed_label(&self) -> &'static str {
        self.record
            .failed_at
            .or_else(|| {
                StepKind::ALL
                    .iter()
                    .copied()
                    .find(|k| self.record.step(*k).status == StepStatus::Failed)
            })
            .map_or("startup", |kind| kind.label_for(self.backend()))
    }

    /// What the step now running is called, while the start runs.
    fn running_label(&self) -> Option<&'static str> {
        self.record
            .current()
            .or_else(|| {
                StepKind::ALL
                    .iter()
                    .copied()
                    .find(|k| self.record.step(*k).status == StepStatus::Running)
            })
            .map(|kind| kind.running_label_for(self.backend()))
    }

    /// The headline of the transcript status row.
    pub(crate) fn headline(&self) -> String {
        match self.phase {
            StartPhase::Starting => match self.running_label() {
                Some(label) => format!("Starting {} \u{b7} {label}", self.noun()),
                None => format!("Starting {}", self.noun()),
            },
            StartPhase::Failed => format!("{} failed at {}", self.title(), self.failed_label()),
            StartPhase::Ready => {
                let total = self.ready.as_ref().map_or_else(
                    || self.record.elapsed_ms(monocode_core::time::now_ms()),
                    |ready| ready.total_ms,
                );
                format!(
                    "{} ready in {}",
                    self.title(),
                    crate::shell::live_elapsed(total)
                )
            }
        }
    }

    /// The composer's phase chip: its text and tone. `None` once the codespace is ready.
    pub(crate) fn chip(&self) -> Option<(String, StartPhase)> {
        match self.phase {
            StartPhase::Starting => Some((
                self.running_label()
                    .map_or_else(|| format!("Starting {}", self.noun()), str::to_owned),
                StartPhase::Starting,
            )),
            StartPhase::Failed => Some(("Startup failed".into(), StartPhase::Failed)),
            StartPhase::Ready => None,
        }
    }

    /// How far along the start is, from 0 to under 1. Counts the steps that block `Ready` (a
    /// finished or skipped one fully, the running one by half) out of one more than there are,
    /// so the bar never fills before the codespace is ready. Steps only move forward, so the
    /// fraction only grows within a start.
    pub(crate) fn progress(&self) -> f32 {
        let blocking: Vec<_> = StepKind::ALL
            .iter()
            .filter(|k| **k != StepKind::LifecycleCommands)
            .collect();
        let credit: f32 = blocking
            .iter()
            .map(|k| match self.record.step(**k).status {
                StepStatus::Done | StepStatus::Skipped => 1.,
                StepStatus::Running => 0.5,
                StepStatus::Pending | StepStatus::Failed => 0.,
            })
            .sum();
        (credit / (blocking.len() as f32 + 1.)).clamp(0.03, 0.97)
    }

    /// The note under the queued message.
    pub(crate) fn queued_note(&self) -> Option<String> {
        match self.phase {
            StartPhase::Starting => {
                Some(format!("Queued, sends when the {} is ready", self.noun()))
            }
            StartPhase::Failed => Some(format!("Not sent, the {} failed to start", self.noun())),
            StartPhase::Ready => None,
        }
    }

    /// The composer placeholder to use on top of the app's own `base`.
    pub(crate) fn placeholder(&self, base: &str) -> Option<String> {
        match self.phase {
            StartPhase::Starting => {
                Some(format!("{base} (sent when the {} is ready)", self.noun()))
            }
            StartPhase::Failed => Some(format!("{base} (sent after a successful retry)")),
            StartPhase::Ready => None,
        }
    }

    /// The text of the failure box: the reason, then why a retry is safe.
    pub(crate) fn failure_message(&self) -> String {
        let reason = self
            .short_error()
            .filter(|reason| !reason.is_empty())
            .unwrap_or_else(|| format!("The {} failed to start.", self.noun()));
        format!(
            "{reason} The {} is kept, so a retry reuses it.",
            self.noun()
        )
    }

    /// The label of the button that deletes the workspace.
    pub(crate) fn delete_label(&self) -> String {
        format!("Delete {}", self.noun())
    }

    /// The error without the advice `created_note` appends (the status row says it once, and
    /// offers the buttons).
    pub(crate) fn short_error(&self) -> Option<String> {
        let error = self.error()?;
        let cut = error
            .find(" (codespace ")
            .filter(|at| error[*at..].contains("was created and kept"))
            .unwrap_or(error.len());
        Some(error[..cut].trim().to_owned())
    }
}

/// Appends `line` to a capped log.
pub(crate) fn push_log(log: &mut Arc<Vec<String>>, line: String) {
    let lines = Arc::make_mut(log);
    lines.push(line);
    if lines.len() > LOG_CAP + 200 {
        let excess = lines.len() - LOG_CAP;
        lines.drain(..excess);
    }
}

/// Milliseconds a step has taken (or is taking at `now`), when it started.
pub(crate) fn step_ms(step: &Step, now: u64) -> Option<u64> {
    let started = step.started_at?;
    if started == 0 {
        return None;
    }
    let elapsed = match step.status {
        StepStatus::Running => Some(now.saturating_sub(started)),
        _ => step.finished_at.map(|end| end.saturating_sub(started)),
    };
    elapsed.filter(|ms| *ms <= 24 * 60 * 60 * 1000)
}

/// A Retry, Rebuild, Open in browser or Delete the user chose for a start, or a request to show
/// the Codespace panel. Conversations emit it; the shell performs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartAction {
    Retry,
    Rebuild,
    OpenInBrowser,
    DeleteCodespace,
    /// Raised by "View log" and the composer chip.
    ShowPanel,
    /// The sandbox of a session is deleted or its container was recreated: archive the session.
    ArchiveSession,
    /// The same: start a new cloud session in the project.
    NewSession,
}

pub(crate) struct CloudStartAction {
    pub action: StartAction,
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// A record whose first `done` steps (in display order) are finished, the next one running.
    pub(crate) fn record(done: usize) -> StartRecord {
        let steps = StepKind::ALL
            .iter()
            .enumerate()
            .map(|(at, kind)| Step {
                kind: *kind,
                status: match at.cmp(&done) {
                    std::cmp::Ordering::Less => StepStatus::Done,
                    std::cmp::Ordering::Equal => StepStatus::Running,
                    std::cmp::Ordering::Greater => StepStatus::Pending,
                },
                started_at: (at <= done).then_some(1_000 + at as u64 * 10_000),
                finished_at: (at < done).then_some(1_000 + at as u64 * 10_000 + 5_000),
                detail: None,
            })
            .collect();
        StartRecord {
            session_id: 1,
            local_project: "/work/widgets".into(),
            codespace: "octo-widgets-x7q9".into(),
            sandbox: None,
            repo: "octo/widgets".into(),
            branch: Some("main".into()),
            machine: None,
            display: "octo/widgets".into(),
            remote_cwd: "/workspaces/widgets".into(),
            web_url: None,
            created: true,
            gh_state: Some("Available".into()),
            steps,
            error: None,
            failed_at: None,
            log_tail: vec![],
            started_at: 1_000,
            finished_at: None,
            track_ready: true,
            sandbox_start: None,
        }
    }

    /// The same snapshot as a Docker sandbox start: the record names a sandbox, not a codespace.
    pub(crate) fn as_sandbox(snapshot: Arc<CloudStartSnapshot>) -> Arc<CloudStartSnapshot> {
        let mut snapshot = (*snapshot).clone();
        snapshot.record.sandbox = Some("claude-widgets-ab12".into());
        snapshot.record.codespace = String::new();
        Arc::new(snapshot)
    }

    pub(crate) fn starting(done: usize) -> Arc<CloudStartSnapshot> {
        Arc::new(CloudStartSnapshot {
            record: record(done),
            phase: StartPhase::Starting,
            log: Arc::new(vec!["Setting up chromium ...".into()]),
            ready: None,
            running: true,
        })
    }

    pub(crate) fn failed(done: usize, error: &str) -> Arc<CloudStartSnapshot> {
        let mut record = record(done);
        let kind = StepKind::ALL[done];
        record.step_status(kind, StepStatus::Failed);
        record.error = Some(error.into());
        record.failed_at = Some(kind);
        record.finished_at = Some(61_000);
        Arc::new(CloudStartSnapshot {
            record,
            phase: StartPhase::Failed,
            log: Arc::new(vec![]),
            ready: None,
            running: false,
        })
    }

    pub(crate) fn ready(total_ms: u64) -> Arc<CloudStartSnapshot> {
        let mut record = record(StepKind::ALL.len() - 1);
        for step in &mut record.steps {
            step.status = StepStatus::Done;
        }
        Arc::new(CloudStartSnapshot {
            ready: Some(ReadySummary {
                total_ms,
                steps: record.steps.clone(),
            }),
            record,
            phase: StartPhase::Ready,
            log: Arc::new(vec![]),
            running: false,
        })
    }

    trait SetStatus {
        fn step_status(&mut self, kind: StepKind, status: StepStatus);
    }
    impl SetStatus for StartRecord {
        fn step_status(&mut self, kind: StepKind, status: StepStatus) {
            self.steps[StepKind::ALL.iter().position(|k| *k == kind).unwrap()].status = status;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    #[test]
    fn headline_names_the_phase_the_failure_and_the_total() {
        // Create and Provision are done, Build container runs.
        assert_eq!(
            starting(2).headline(),
            "Starting codespace \u{b7} Building container"
        );
        assert_eq!(
            failed(3, "cargo fetch exited with code 101").headline(),
            "Codespace failed at Lifecycle commands (waitFor)"
        );
        assert_eq!(ready(554_000).headline(), "Codespace ready in 9m 14s");
    }

    #[test]
    fn a_sandbox_start_uses_the_sandbox_words_everywhere() {
        let first = as_sandbox(starting(0));
        assert_eq!(first.backend(), CloudBackendKind::DockerSandbox);
        assert_eq!(first.headline(), "Starting sandbox \u{b7} Creating sandbox");
        assert_eq!(
            as_sandbox(starting(4)).headline(),
            "Starting sandbox \u{b7} Configuring environment"
        );
        assert_eq!(
            as_sandbox(failed(4, "boom")).headline(),
            "Sandbox failed at Configure environment"
        );
        assert_eq!(
            as_sandbox(ready(554_000)).headline(),
            "Sandbox ready in 9m 14s"
        );
        assert_eq!(
            first.chip(),
            Some(("Creating sandbox".into(), StartPhase::Starting))
        );
        assert_eq!(
            as_sandbox(failed(2, "boom")).chip(),
            Some(("Startup failed".into(), StartPhase::Failed))
        );
        assert_eq!(
            first.queued_note().as_deref(),
            Some("Queued, sends when the sandbox is ready")
        );
        assert_eq!(
            as_sandbox(failed(2, "boom")).queued_note().as_deref(),
            Some("Not sent, the sandbox failed to start")
        );
        let base = "Ask";
        assert_eq!(
            first.placeholder(base).unwrap(),
            "Ask (sent when the sandbox is ready)"
        );
        assert_eq!(
            as_sandbox(failed(2, "boom")).placeholder(base).unwrap(),
            "Ask (sent after a successful retry)"
        );
        assert_eq!(first.title(), "Sandbox");
        assert_eq!(first.delete_label(), "Delete sandbox");
        assert!(!first.offers_browser(), "a sandbox has no web view");
        assert_eq!(
            as_sandbox(failed(2, "cargo fetch failed")).failure_message(),
            "cargo fetch failed The sandbox is kept, so a retry reuses it."
        );
        assert_eq!(
            as_sandbox(failed(2, "")).failure_message(),
            "The sandbox failed to start. The sandbox is kept, so a retry reuses it."
        );
    }

    #[test]
    fn the_codespace_strings_are_unchanged() {
        // Golden strings: a codespace start must read exactly as before sandboxes existed.
        let snapshot = starting(2);
        assert_eq!(snapshot.backend(), CloudBackendKind::Codespaces);
        assert_eq!(snapshot.title(), "Codespace");
        assert_eq!(snapshot.delete_label(), "Delete codespace");
        assert!(snapshot.offers_browser());
        assert_eq!(
            snapshot.headline(),
            "Starting codespace \u{b7} Building container"
        );
        assert_eq!(
            starting(0).headline(),
            "Starting codespace \u{b7} Creating codespace"
        );
        assert_eq!(
            failed(4, "x").headline(),
            "Codespace failed at Configure codespace"
        );
        assert_eq!(ready(1000).headline(), "Codespace ready in 1s");
        assert_eq!(
            failed(2, "cargo fetch failed").failure_message(),
            "cargo fetch failed The codespace is kept, so a retry reuses it."
        );
        assert_eq!(
            failed(2, "").failure_message(),
            "The codespace failed to start. The codespace is kept, so a retry reuses it."
        );
        assert_eq!(
            snapshot.queued_note().as_deref(),
            Some("Queued, sends when the codespace is ready")
        );
        assert_eq!(
            snapshot.placeholder("Ask").unwrap(),
            "Ask (sent when the codespace is ready)"
        );
    }

    #[test]
    fn progress_only_grows_and_stays_below_full_while_starting() {
        let mut last = 0.;
        for done in 0..StepKind::ALL.len() {
            let fraction = starting(done).progress();
            assert!(fraction >= last, "{done}: {fraction} < {last}");
            assert!(fraction < 1., "{done}: {fraction}");
            last = fraction;
        }
        // Every step that blocks Ready finished, the background one not started.
        let mut nearly = (*starting(6)).clone();
        nearly.record.steps[6].status = StepStatus::Done;
        assert!(nearly.progress() < 1.);
        assert!(nearly.progress() > 0.8);
    }

    #[test]
    fn chip_follows_the_phase_and_is_gone_when_ready() {
        assert_eq!(
            starting(2).chip(),
            Some(("Building container".into(), StartPhase::Starting))
        );
        assert_eq!(
            failed(2, "boom").chip(),
            Some(("Startup failed".into(), StartPhase::Failed))
        );
        assert_eq!(ready(1000).chip(), None);
    }

    #[test]
    fn queued_note_and_placeholder_follow_the_phase() {
        let base = "Ask, build, or describe a change\u{2026}";
        assert_eq!(
            starting(2).queued_note().as_deref(),
            Some("Queued, sends when the codespace is ready")
        );
        assert_eq!(
            failed(2, "boom").queued_note().as_deref(),
            Some("Not sent, the codespace failed to start")
        );
        assert_eq!(ready(1000).queued_note(), None);
        assert_eq!(
            starting(2).placeholder(base).unwrap(),
            format!("{base} (sent when the codespace is ready)")
        );
        assert_eq!(
            failed(2, "boom").placeholder(base).unwrap(),
            format!("{base} (sent after a successful retry)")
        );
        assert_eq!(ready(1000).placeholder(base), None);
    }

    #[test]
    fn short_error_drops_the_retry_advice() {
        let note = monocode_core::cloud_sessions::created_note("octo-1", "cargo fetch failed");
        let snapshot = failed(2, &note);
        assert_eq!(snapshot.short_error().unwrap(), "cargo fetch failed");
        assert_eq!(failed(2, "plain").short_error().unwrap(), "plain");
    }
    #[test]
    fn missing_or_implausible_step_times_never_show_a_duration() {
        let mut step = record(2).steps[2].clone();
        step.started_at = None;
        assert_eq!(step_ms(&step, 1000), None);
        step.started_at = Some(0);
        assert_eq!(step_ms(&step, 1000), None);
        step.started_at = Some(1000);
        assert_eq!(step_ms(&step, 90_000_000), None);
        assert_eq!(step_ms(&step, 31_000), Some(30_000));
    }
}
