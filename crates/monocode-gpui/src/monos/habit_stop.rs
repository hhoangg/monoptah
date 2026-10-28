//! Stopping a running habit: the three-choice sheet, the request, and what the
//! stopped run does with the sessions it started.
use super::*;

pub(super) const STOPPED_BY_USER: &str = "Stopped by the user.";
pub(super) const CHOICES: [&str; 3] =
    ["Stop run only", "Stop run and its sessions", "Keep running"];
/// The sheet's buttons; "Keep running" is the cancel one, so Escape keeps the run going.
pub(super) fn buttons() -> [gpui::PromptButton; 3] {
    [
        gpui::PromptButton::new(CHOICES[0]),
        gpui::PromptButton::new(CHOICES[1]),
        gpui::PromptButton::cancel(CHOICES[2]),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopMode {
    /// End the run; the sessions it started keep running.
    Run,
    /// End the run and stop the sessions it created.
    All,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum StopChoice {
    Kept,
    Requested,
    /// The run ended while the sheet was open.
    Finished,
}

pub(super) fn stop_text(name: &str) -> String {
    format!("Stop \"{name}\"? Sessions it already started can keep running.")
}
pub(super) fn finished_text(name: &str) -> String {
    format!("\"{name}\" already finished, so there was nothing to stop.")
}
/// The mode a button stands for; keeping it running, or closing the sheet, is none.
pub(super) fn mode_for(answer: Option<usize>) -> Option<StopMode> {
    match answer {
        Some(0) => Some(StopMode::Run),
        Some(1) => Some(StopMode::All),
        _ => None,
    }
}
/// Records the request. A turn still working is failed with "Stopped by the user."
/// (which is how the stored outcome is told apart); one that had just finished
/// keeps its own outcome, and the choice still applies to its sessions.
pub(super) fn apply_stop(
    stop: &mut Option<StopMode>,
    failure: &mut Option<String>,
    mode: StopMode,
    busy: bool,
) {
    if stop.is_some() {
        return;
    }
    *stop = Some(mode);
    if busy && failure.is_none() {
        *failure = Some(STOPPED_BY_USER.into());
    }
}
/// A stop asked for before the turn was working still has to end it once it is.
pub(super) fn stop_pending(stop: Option<StopMode>, failure: &Option<String>, busy: bool) -> bool {
    stop.is_some() && failure.is_none() && busy
}
/// What the run posts to the chat: for a run the user stopped, where its sessions
/// are left running; otherwise its report, or what it started. Then any session
/// that would not stop.
pub(super) fn post_text(
    report: Option<&str>,
    mode: Option<StopMode>,
    user_stopped: bool,
    created: usize,
    remaining: usize,
    stuck: &[i64],
) -> Option<String> {
    let text = if user_stopped {
        (mode == Some(StopMode::Run) && created > 0).then(|| {
            let kept = if created == 1 {
                "1 session that keeps".to_string()
            } else {
                format!("{created} sessions that keep")
            };
            format!("Stopped. Started {kept} running.")
        })
    } else {
        scheduler::run_post(report, remaining)
    };
    let stuck = (!stuck.is_empty()).then(|| {
        format!(
            "Could not stop {} {}: {}.",
            stuck.len(),
            if stuck.len() == 1 {
                "session"
            } else {
                "sessions"
            },
            stuck
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    let post = [text, stuck]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    (!post.is_empty()).then_some(post)
}

impl Monos {
    /// The running run of a habit of the open Mono.
    pub(super) fn habit_run(&self, habit: &str) -> Option<i64> {
        self.running
            .iter()
            .find(|(_, r)| r.habit == habit && self.active.as_deref() == Some(r.mono.as_str()))
            .map(|(&session, _)| session)
    }
    /// True from the moment the user asks to stop until the run has ended.
    pub(super) fn habit_stopping(&self, habit: &str) -> bool {
        self.habit_run(habit)
            .is_some_and(|run| self.running[&run].stop.is_some())
    }
    pub(super) fn wants_stop_prompt(&self, habit: &str) -> bool {
        self.habit_run(habit)
            .is_some_and(|run| self.running[&run].stop.is_none())
    }
    /// Asks how to stop the habit's run, then asks the scheduler to stop it.
    pub(super) fn stop_habit(&mut self, habit: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.wants_stop_prompt(habit) {
            return;
        }
        let name = self
            .habits
            .iter()
            .find(|h| h.id == habit)
            .map(|h| h.name.clone())
            .unwrap_or_default();
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            "Monoptah",
            Some(&stop_text(&name)),
            &buttons(),
            cx,
        );
        let habit = habit.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let answer = prompt.await.ok();
            let _ = this.update_in(cx, |this, window, cx| {
                if this.choose_stop(&habit, answer, cx) == StopChoice::Finished {
                    drop(window.prompt(
                        gpui::PromptLevel::Info,
                        "Monoptah",
                        Some(&finished_text(&name)),
                        &["OK"],
                        cx,
                    ));
                }
            });
        })
        .detach();
    }
    /// Applies the sheet's answer. A run that ended while the sheet was open has
    /// nothing left to stop, and a request recorded then would be dropped unseen.
    pub(super) fn choose_stop(
        &mut self,
        habit: &str,
        answer: Option<usize>,
        cx: &mut Context<Self>,
    ) -> StopChoice {
        let Some(mode) = mode_for(answer) else {
            return StopChoice::Kept;
        };
        let Some(session) = self.habit_run(habit) else {
            return StopChoice::Finished;
        };
        let run = self.running.get_mut(&session).unwrap();
        let busy = run.view.read(cx).busy;
        apply_stop(&mut run.stop, &mut run.failure, mode, busy);
        if busy {
            run.view.update(cx, |v, cx| v.mono_stop(cx));
        }
        self.changed(cx);
        StopChoice::Requested
    }
    /// Ends a run whose stop was requested before its turn started working.
    pub(super) fn enforce_stop(&mut self, session: i64, cx: &mut Context<Self>) {
        let Some(run) = self.running.get_mut(&session) else {
            return;
        };
        if stop_pending(run.stop, &run.failure, run.view.read(cx).busy) {
            run.failure = Some(STOPPED_BY_USER.into());
            run.view.update(cx, |v, cx| v.mono_stop(cx));
        }
    }
    /// Stops a session a run created and drops its pending completion report.
    /// Mono chats and habit runs are never stopped from a habit.
    pub(super) fn stop_created(
        &mut self,
        mono: &str,
        session: i64,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.is_mono_session(session) {
            return Err("The session cannot be stopped from a habit".into());
        }
        let view = self
            .owner
            .upgrade()
            .and_then(|owner| owner.update(cx, |owner, cx| owner.mono_ensure_session(session, cx)))
            .ok_or("The session is not open")?;
        view.update(cx, |v, cx| v.stop_if_busy(cx));
        if let Some(mono) = self.roster.get_mut(mono) {
            for launch in mono.launches.iter_mut().filter(|l| l.session_id == session) {
                launch.dismissed = true;
            }
        }
        self.save(cx);
        Ok(())
    }
}
