//! The "Continue in cloud" dialog (design 4.2.2 dialog, 4.5): ONE confirmation that lists the
//! target, the branch that will be pushed, the counts, the ignored files that stay on this
//! computer and the warnings; then the progress of the move (the steps of a `MoveRecord`), with
//! Retry and Cancel.
//!
//! The view holds no engine and starts nothing. The shell feeds it (`show_preview`,
//! `update_record`, ...) and performs what it emits ([`Event`]). In particular the only way to
//! start a move is [`Event::Confirm`], which the Continue button raises once.
use crate::{
    cloud_dialog_ui::{alert, choice, heading, labelled, muted},
    modal,
    theme::*,
    transcript::cloud_start::step_row,
};
use gpui::{
    AnyElement, Context, Div, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    Render, Role, SharedString, Window, div, prelude::*, px,
};
use monocode_core::cloud_sessions::{
    StepStatus,
    move_session::{CancelReport, MoveRecord, MoveStep, MoveTarget},
    move_ui::MovePreview,
};

/// What the dialog asks the shell to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// Close the dialog. A running move goes on in the background.
    Close,
    /// The user confirmed. The only way a move starts.
    Confirm(MoveTarget),
    /// Continue at the failed step (or resume an interrupted move).
    Retry,
    /// Roll the move back. `delete_workspace` also deletes a workspace the move created.
    CancelMove { delete_workspace: bool },
}

/// One place the session can go.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TargetChoice {
    pub label: String,
    pub detail: String,
    pub target: MoveTarget,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Stage {
    /// Reading the working copy (a fetch and a snapshot, nothing is changed).
    Loading,
    /// Nothing can start: busy, no origin, a detached HEAD, ...
    Refused(String),
    /// The one confirmation.
    Review(Box<MovePreview>),
    /// A worker runs the steps.
    Running,
    /// A step failed, or the move was found unfinished after a restart.
    Paused,
    /// A cancel was requested; the rollback runs.
    Rolling,
    Cancelled(CancelReport),
    Done,
}

pub(crate) struct MoveDialog {
    title: String,
    /// What the conversation does in the cloud (resume, or a summary and a new Claude session).
    conversation: String,
    targets: Vec<TargetChoice>,
    selected: usize,
    stage: Stage,
    record: Option<MoveRecord>,
    notices: Vec<String>,
    error: Option<String>,
    focus: FocusHandle,
}

impl EventEmitter<Event> for MoveDialog {}

impl Focusable for MoveDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

// ------------------------------------------------------------------------ text

pub(crate) fn bytes_label(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.;
    let mut unit = "KB";
    for next in ["MB", "GB"] {
        if value < 1024. {
            break;
        }
        value /= 1024.;
        unit = next;
    }
    if value < 10. {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "Push N commits to origin/<branch>": what the push does, in words.
pub(crate) fn push_line(p: &MovePreview) -> String {
    if !p.needs_push {
        return format!(
            "Nothing to push: origin already has this commit on {}.",
            p.branch
        );
    }
    let commits = plural(p.commits, "commit", "commits");
    if p.creates_branch && p.branch != p.local_branch && !p.local_branch.is_empty() {
        format!(
            "Push {commits} to a new branch, origin/{}. {} is not pushed as it is: it has diverged from origin or is origin's default branch.",
            p.branch, p.local_branch
        )
    } else if p.creates_branch {
        format!("Push {commits} to a new branch, origin/{}.", p.branch)
    } else {
        format!("Push {commits} to origin/{}.", p.branch)
    }
}

/// "M changed and K new files go along uncommitted".
pub(crate) fn changes_line(p: &MovePreview) -> String {
    if p.changed + p.added == 0 {
        return "No uncommitted changes.".into();
    }
    format!(
        "{} and {} go along as uncommitted changes (patch {}). They arrive as unstaged changes.",
        plural(p.changed, "changed file", "changed files"),
        plural(p.added, "new file", "new files"),
        bytes_label(p.patch_bytes)
    )
}

/// The ignored files that stay local, `None` when there are none.
pub(crate) fn ignored_line(p: &MovePreview) -> Option<String> {
    if p.ignored.is_empty() {
        return None;
    }
    const SHOWN: usize = 6;
    let names: Vec<String> = p
        .ignored
        .iter()
        .take(SHOWN)
        .map(|(name, bytes)| format!("{name} ({})", bytes_label(*bytes)))
        .collect();
    let more = p.ignored.len().saturating_sub(SHOWN);
    let mut text = format!(
        "Ignored files stay on this computer ({}): {}",
        bytes_label(p.ignored_bytes),
        names.join(", ")
    );
    if more > 0 {
        text.push_str(&format!(" and {more} more"));
    }
    text.push('.');
    Some(text)
}

/// LFS and dirty-submodule warnings.
pub(crate) fn warning_lines(p: &MovePreview) -> Vec<String> {
    let mut lines = Vec::new();
    if p.lfs {
        lines.push(
            "This project uses Git LFS. A branch push may not carry the contents of LFS files."
                .into(),
        );
    }
    if !p.dirty_submodules.is_empty() {
        lines.push(format!(
            "Changes inside submodules do not travel: {}.",
            p.dirty_submodules.join(", ")
        ));
    }
    lines
}

/// The status of every step of a move, in order.
pub(crate) fn step_statuses(record: &MoveRecord) -> Vec<(MoveStep, StepStatus)> {
    MoveStep::ALL
        .iter()
        .map(|step| {
            let status = if record.done.contains(step) {
                StepStatus::Done
            } else if record.failed_at == Some(*step) {
                StepStatus::Failed
            } else if record.current == Some(*step) {
                StepStatus::Running
            } else {
                StepStatus::Pending
            };
            (*step, status)
        })
        .collect()
}

/// The lines under a cancelled move: what is left for the user to decide.
pub(crate) fn cancel_lines(report: &CancelReport) -> Vec<String> {
    let mut lines =
        vec!["The move was cancelled. This conversation is still on this computer.".into()];
    if let Some(branch) = &report.created_branch {
        lines.push(format!(
            "The branch {branch} was pushed to origin and is kept. Delete it there if you do not need it."
        ));
    }
    if let Some(cloud) = &report.kept_workspace {
        lines.push(format!(
            "The {} {} was kept.",
            cloud.kind.noun(),
            cloud.name
        ));
    }
    if report.deleted_workspace {
        lines.push("The workspace the move created was deleted.".into());
    }
    lines.extend(report.warnings.iter().cloned());
    lines
}

// ------------------------------------------------------------------------ view

impl MoveDialog {
    pub(crate) fn new(
        title: String,
        conversation: String,
        targets: Vec<TargetChoice>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            title,
            conversation,
            targets,
            selected: 0,
            stage: Stage::Loading,
            record: None,
            notices: Vec::new(),
            error: None,
            focus: cx.focus_handle(),
        }
    }

    #[cfg(test)]
    pub(crate) fn stage(&self) -> &Stage {
        &self.stage
    }
    #[cfg(test)]
    pub(crate) fn record(&self) -> Option<&MoveRecord> {
        self.record.as_ref()
    }
    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The answer of the read-only inspection.
    pub(crate) fn show_preview(
        &mut self,
        preview: Result<MovePreview, String>,
        cx: &mut Context<Self>,
    ) {
        if self.stage != Stage::Loading {
            return;
        }
        self.stage = match preview {
            Ok(preview) => Stage::Review(Box::new(preview)),
            Err(error) => Stage::Refused(error),
        };
        cx.notify();
    }

    /// The move was refused before it started (busy, already moving).
    pub(crate) fn refuse(&mut self, error: String, cx: &mut Context<Self>) {
        self.stage = Stage::Refused(error);
        cx.notify();
    }

    pub(crate) fn select_target(&mut self, ix: usize, cx: &mut Context<Self>) {
        if matches!(self.stage, Stage::Review(_)) && ix < self.targets.len() {
            self.selected = ix;
            cx.notify();
        }
    }

    /// The latest record of the move. A failed record pauses the dialog on Retry and Cancel.
    pub(crate) fn update_record(&mut self, record: MoveRecord, cx: &mut Context<Self>) {
        if record.is_failed() {
            self.error = record.error.clone();
            if !matches!(
                self.stage,
                Stage::Rolling | Stage::Cancelled(_) | Stage::Done
            ) {
                self.stage = Stage::Paused;
            }
        } else if matches!(
            self.stage,
            Stage::Review(_) | Stage::Loading | Stage::Paused
        ) {
            self.error = None;
            self.stage = Stage::Running;
        }
        self.notices = record.notices.clone();
        self.record = Some(record);
        cx.notify();
    }

    /// A move found unfinished (after a restart, or its dialog was closed): nothing runs until
    /// the user presses Retry or Cancel.
    pub(crate) fn show_unfinished(&mut self, record: MoveRecord, cx: &mut Context<Self>) {
        self.error = record.error.clone();
        self.notices = record.notices.clone();
        self.record = Some(record);
        self.stage = Stage::Paused;
        cx.notify();
    }

    pub(crate) fn push_notice(&mut self, notice: String, cx: &mut Context<Self>) {
        if !self.notices.contains(&notice) {
            self.notices.push(notice);
            cx.notify();
        }
    }

    pub(crate) fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        if !matches!(
            self.stage,
            Stage::Rolling | Stage::Cancelled(_) | Stage::Done
        ) {
            self.stage = Stage::Paused;
        }
        cx.notify();
    }

    pub(crate) fn cancelled(&mut self, report: CancelReport, cx: &mut Context<Self>) {
        self.stage = Stage::Cancelled(report);
        cx.notify();
    }

    pub(crate) fn done(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.stage = Stage::Done;
        cx.notify();
    }

    /// A move of this session is running already (its dialog was closed): show it as running.
    pub(crate) fn show_running(&mut self, cx: &mut Context<Self>) {
        self.stage = Stage::Running;
        cx.notify();
    }

    /// The Continue button. Raises [`Event::Confirm`] once; a second press does nothing.
    pub(crate) fn confirm(&mut self, cx: &mut Context<Self>) {
        let Stage::Review(_) = &self.stage else {
            return;
        };
        let Some(choice) = self.targets.get(self.selected) else {
            return;
        };
        let target = choice.target.clone();
        // The stage leaves Review at once, so a double click cannot confirm twice.
        self.stage = Stage::Running;
        self.error = None;
        cx.notify();
        cx.emit(Event::Confirm(target));
    }

    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        if self.stage != Stage::Paused || self.record.as_ref().is_some_and(|r| r.stale) {
            return;
        }
        self.error = None;
        self.stage = Stage::Running;
        cx.notify();
        cx.emit(Event::Retry);
    }

    pub(crate) fn cancel_move(&mut self, delete_workspace: bool, cx: &mut Context<Self>) {
        if !matches!(self.stage, Stage::Paused | Stage::Running) {
            return;
        }
        self.stage = Stage::Rolling;
        cx.notify();
        cx.emit(Event::CancelMove { delete_workspace });
    }

    fn line(text: impl Into<SharedString>) -> Div {
        div()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(content(0.75))
            .child(text.into())
    }

    fn lines(id: &'static str, label: &'static str, lines: Vec<String>) -> Div {
        let mut column = labelled(label).gap(px(4.));
        for (ix, text) in lines.into_iter().enumerate() {
            column = column.child(
                Self::line(text)
                    .id(SharedString::from(format!("{id}-{ix}")))
                    .debug_selector(move || format!("{id}-{ix}")),
            );
        }
        column
    }

    fn review(&self, preview: &MovePreview, cx: &mut Context<Self>) -> Div {
        let mut target = labelled("Where").gap(px(2.));
        if self.targets.is_empty() {
            target = target.child(alert(
                "move-no-target",
                "No cloud target is available for this project. Connect a GitHub repository or set up a Docker Sandbox first.",
            ));
        }
        for (ix, choice_) in self.targets.iter().enumerate() {
            let text = if choice_.detail.is_empty() {
                choice_.label.clone()
            } else {
                format!("{} - {}", choice_.label, choice_.detail)
            };
            target = target.child(
                choice(format!("move-target-{ix}"), text, ix == self.selected)
                    .on_click(cx.listener(move |this, _, _, cx| this.select_target(ix, cx))),
            );
        }
        let mut what = vec![push_line(preview), changes_line(preview)];
        what.extend(ignored_line(preview));
        let warnings = warning_lines(preview);
        let mut column = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(target)
            .child(Self::lines("move-what", "What will move", what))
            .child(Self::lines(
                "move-conversation",
                "Conversation",
                vec![self.conversation.clone()],
            ));
        if !warnings.is_empty() {
            column = column.child(Self::lines("move-warning", "Heads up", warnings));
        }
        column.child(muted(
            "The conversation (which may contain pasted secrets) and the code are stored in the cloud workspace. Nothing is force-pushed, and origin's default branch is never changed.",
        ))
    }

    fn steps(&self) -> Div {
        let mut column = div().flex().flex_col();
        let Some(record) = &self.record else {
            return column;
        };
        for (step, status) in step_statuses(record) {
            let note = (status == StepStatus::Failed).then(|| "failed".to_owned());
            column = column.child(step_row(
                SharedString::from(format!("move-step-{step:?}")),
                step.label(),
                status,
                note,
            ));
        }
        column
    }

    fn progress(&self) -> Div {
        let mut column = div().flex().flex_col().gap_3().child(self.steps());
        if let Some(error) = &self.error {
            column = column.child(alert("move-error", error.clone()));
        }
        if !self.notices.is_empty() {
            column = column.child(Self::lines("move-notice", "Notes", self.notices.clone()));
        }
        if self.stage == Stage::Running {
            column = column.child(muted(
                "You can close this window. The move goes on and can be resumed from the conversation's menu.",
            ));
        }
        column
    }

    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let close = |label: &'static str| {
            modal::ghost_button("move-close", label)
                .debug_selector(|| "move-close".into())
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close)))
        };
        let mut row = modal::actions().mt(px(0.));
        match &self.stage {
            Stage::Loading => row = row.child(close("Cancel")),
            Stage::Refused(_) | Stage::Cancelled(_) | Stage::Done => {
                row = row.child(close("Close"))
            }
            Stage::Review(_) => {
                row = row.child(close("Not now")).child(
                    modal::primary_button("move-confirm", "Continue in cloud")
                        .debug_selector(|| "move-confirm".into())
                        .cursor_pointer()
                        .when(self.targets.is_empty(), |d| d.opacity(0.4))
                        .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                );
            }
            Stage::Rolling => row = row.child(close("Close")),
            Stage::Running | Stage::Paused => {
                let created = self
                    .record
                    .as_ref()
                    .and_then(|r| r.workspace.as_ref())
                    .filter(|w| w.created && !w.name.is_empty())
                    .map(|w| w.backend.noun());
                row = row.child(close("Close"));
                if let Some(noun) = created {
                    row = row.child(
                        modal::danger_ghost_button(
                            "move-cancel-delete",
                            format!("Cancel and delete {noun}"),
                        )
                        .debug_selector(|| "move-cancel-delete".into())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_move(true, cx))),
                    );
                }
                row = row.child(
                    modal::ghost_button("move-cancel", "Cancel move")
                        .debug_selector(|| "move-cancel".into())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_move(false, cx))),
                );
                let stale = self.record.as_ref().is_some_and(|r| r.stale);
                if self.stage == Stage::Paused && !stale {
                    let label = if self.error.is_some() {
                        "Retry"
                    } else {
                        "Resume"
                    };
                    row = row.child(
                        modal::primary_button("move-retry", label)
                            .debug_selector(|| "move-retry".into())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    );
                }
            }
        }
        div()
            .flex_shrink_0()
            .px_4()
            .pb_4()
            .pt_3()
            .border_t_1()
            .border_color(content(0.08))
            .child(row)
    }
}

impl Render for MoveDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = window.viewport_size().height.as_f32() - 34.;
        let body: AnyElement = match self.stage.clone() {
            Stage::Loading => muted("Reading your working copy...").into_any_element(),
            Stage::Refused(error) => alert("move-refused", error).into_any_element(),
            Stage::Review(preview) => self.review(&preview, cx).into_any_element(),
            Stage::Running | Stage::Paused => self.progress().into_any_element(),
            Stage::Rolling => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(self.steps())
                .child(muted("Rolling back what the move created..."))
                .into_any_element(),
            Stage::Cancelled(report) => {
                Self::lines("move-cancelled", "Cancelled", cancel_lines(&report)).into_any_element()
            }
            Stage::Done => Self::lines(
                "move-done",
                "Done",
                vec!["This conversation now continues in the cloud.".into()],
            )
            .into_any_element(),
        };
        div()
            .id("move-dialog")
            .debug_selector(|| "move-dialog".into())
            .role(Role::Dialog)
            .aria_label("Continue in cloud")
            .track_focus(&self.focus)
            .relative()
            .min_h_0()
            .max_h(px(height))
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(Event::Close);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_4()
                    .pt_3()
                    .child(heading("Continue in cloud", self.title.clone()))
                    .child(
                        icon_button("move-x", "Close", "close")
                            .size(px(28.))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    ),
            )
            .child(
                div()
                    .id("move-dialog-body")
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .child(body),
            )
            .child(self.footer(cx))
    }
}
