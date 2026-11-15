//! The dialog that stands between a Mono and a shell: a free `terminals.open command` or
//! `terminals.send text` waits here until the user decides (see docs/agent-access.md).
//!
//! Rules this file keeps:
//! - Showing the dialog runs nothing. Only Run or Allow writes, and both re-run the call's
//!   checks first (`Shell::mono_terminals_approved`).
//! - One dialog at a time; later requests wait in a queue and each answers its own call.
//! - Deny is the focused button, Escape denies, and Enter chooses the focused button. The
//!   other two buttons ignore keys and clicks for `GUARD` after the dialog appears, so a
//!   keystroke or click aimed at what was on screen a moment ago cannot approve anything.
//! - A request that outlives `APPROVAL_WAIT`, or whose terminal, Mono or project went away,
//!   is answered with an error and dropped. The bridge waits longer than `APPROVAL_WAIT`, so a
//!   call is always answered while its Mono still waits, and one whose Mono stopped waiting
//!   (its channel closed) is dropped without running.
use super::mono_terminals::{MonoTerminalReply, Request};
use super::*;
use gpui::{MouseButton, anchored, deferred};
use serde_json::Value;
use std::{collections::VecDeque, time::Duration};

/// How long the Run and Allow buttons ignore the user after the dialog appears.
pub(super) const GUARD: Duration = Duration::from_millis(300);
/// How often the queue is checked against the terminals, Monos and projects it names.
const WATCH: Duration = Duration::from_millis(500);
pub(super) const DECLINED: &str = "The user declined this command";
const EXPIRED: &str =
    "The user did not answer in time, so nothing was run. Ask again only if it is still needed";
const GONE: &str = "This Mono or its project is no longer available";

/// The buttons, left to right. The focused one starts on Deny.
const DENY: usize = 0;
const ALLOW: usize = 1;
const RUN: usize = 2;

pub(super) struct Approval {
    pub(super) seq: u64,
    pub(super) mono: String,
    pub(super) mono_name: String,
    pub(super) project: String,
    pub(super) request: Request,
    pub(super) reply: async_channel::Sender<Result<Value, String>>,
    /// The dialog has been put in front of the user (it is the first in the queue).
    pub(super) shown: bool,
    /// Run and Allow accept input.
    pub(super) armed: bool,
    /// The deadline timer was started.
    pub(super) timed: bool,
    pub(super) selected: usize,
}

#[derive(Default)]
pub(super) struct Approvals {
    pub(super) queue: VecDeque<Approval>,
    pub(super) next: u64,
    pub(super) watching: bool,
    pub(super) focus: Option<FocusHandle>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Choice {
    Run,
    Allow,
    Deny,
    Expired,
    /// The call cannot run any more, for this reason.
    Gone(String),
}

/// The text of a command or typed text with every character the user could not otherwise
/// see written out: `\n` (and a real line break after it), `\t`, `\x1b`-style escapes for the
/// other control characters, and `\u{..}` for invisible or direction-changing ones.
pub(super) fn visible(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_ascii_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if c.is_control() || invisible(c) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Format and space characters that draw nothing, or draw the text in another order.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{a0}' | '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{1680}' | '\u{180e}'
            | '\u{2000}'..='\u{200f}'
            | '\u{2028}'..='\u{202f}'
            | '\u{205f}'..='\u{206f}'
            | '\u{3000}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}'
    )
}

impl Shell {
    pub(super) fn queue_mono_approval(
        &mut self,
        mono: &str,
        project: &str,
        request: Request,
        cx: &mut Context<Self>,
    ) -> MonoTerminalReply {
        let Some(mono_name) = self
            .monos
            .as_ref()
            .and_then(|monos| monos.read(cx).terminal_caller(mono, project))
        else {
            return MonoTerminalReply::Now(Err(GONE.into()));
        };
        let (reply, answer) = async_channel::bounded(1);
        self.mono_approvals.next += 1;
        let seq = self.mono_approvals.next;
        self.mono_approvals.queue.push_back(Approval {
            seq,
            mono: mono.to_owned(),
            mono_name,
            project: project.to_owned(),
            request,
            reply,
            shown: false,
            armed: false,
            timed: false,
            selected: DENY,
        });
        MonoTerminalReply::Later(answer)
    }

    /// Puts the first waiting request in front of the user, and starts the timers of the new
    /// ones. Called with a window after every `terminals.*` call.
    pub(crate) fn show_mono_approval(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let waiting: Vec<u64> = self
            .mono_approvals
            .queue
            .iter_mut()
            .filter_map(|a| (!std::mem::replace(&mut a.timed, true)).then_some(a.seq))
            .collect();
        for seq in waiting {
            let deadline = cx
                .background_executor()
                .timer(monocode_core::mono::bridge::APPROVAL_WAIT);
            cx.spawn_in(window, async move |this, cx| {
                deadline.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.answer_mono_approval(seq, Choice::Expired, window, cx)
                });
            })
            .detach();
        }
        if self.mono_approvals.queue.is_empty() {
            return;
        }
        if self.mono_approvals.focus.is_none() {
            self.mono_approvals.focus = Some(cx.focus_handle());
        }
        if let Some(front) = self.mono_approvals.queue.front_mut()
            && !front.shown
        {
            front.shown = true;
            front.armed = false;
            front.selected = DENY;
            let (seq, guard) = (front.seq, cx.background_executor().timer(GUARD));
            cx.spawn(async move |this, cx| {
                guard.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(a) = this.mono_approvals.queue.iter_mut().find(|a| a.seq == seq) {
                        a.armed = true;
                        cx.notify();
                    }
                });
            })
            .detach();
            if let Some(focus) = &self.mono_approvals.focus {
                window.focus(focus, cx);
            }
        }
        if !self.mono_approvals.watching {
            self.mono_approvals.watching = true;
            let executor = cx.background_executor().clone();
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    executor.timer(WATCH).await;
                    let more = this
                        .update_in(cx, |this, window, cx| this.check_mono_approvals(window, cx))
                        .unwrap_or(false);
                    if !more {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    /// Drops what can no longer run. Returns whether anything still waits.
    fn check_mono_approvals(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let verdicts: Vec<(u64, Option<String>)> = self
            .mono_approvals
            .queue
            .iter()
            .map(|a| (a.seq, self.approval_problem(a, cx)))
            .collect();
        for (seq, problem) in verdicts {
            if let Some(problem) = problem {
                self.answer_mono_approval(seq, Choice::Gone(problem), window, cx);
            }
        }
        if self.mono_approvals.queue.is_empty() {
            self.mono_approvals.watching = false;
            return false;
        }
        true
    }

    /// Why this request can no longer run, if it cannot: its Mono or project is gone, the
    /// terminal closed, exited or runs an agent, or the directory left the project.
    fn approval_problem(&self, approval: &Approval, cx: &App) -> Option<String> {
        let known = self.monos.as_ref().is_some_and(|monos| {
            monos
                .read(cx)
                .terminal_caller(&approval.mono, &approval.project)
                .is_some()
        });
        if !known {
            return Some(GONE.into());
        }
        self.mono_terminal_ready(
            &approval.mono,
            &approval.project,
            approval.request.action,
            &approval.request.input,
            cx,
        )
        .err()
    }

    /// Chooses for the request `seq`: runs it, remembers it, or answers its call with an
    /// error. Whatever the choice, the request leaves the queue and the next one is shown.
    pub(super) fn answer_mono_approval(
        &mut self,
        seq: u64,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.mono_approvals.queue.iter().position(|a| a.seq == seq) else {
            return;
        };
        let Some(approval) = self.mono_approvals.queue.remove(at) else {
            return;
        };
        let was_front = at == 0;
        let result = match choice {
            Choice::Deny => Err(DECLINED.to_owned()),
            Choice::Expired => Err(EXPIRED.to_owned()),
            Choice::Gone(reason) => Err(reason),
            Choice::Run | Choice::Allow => {
                if approval.reply.is_closed() {
                    // The Mono stopped waiting: a late answer must not run anything.
                    Err("The call was abandoned".to_owned())
                } else if let Some(problem) = self.approval_problem(&approval, cx) {
                    Err(problem)
                } else {
                    if choice == Choice::Allow
                        && let Some(monos) = self.monos.clone()
                    {
                        let (kind, text) = (approval.request.kind, approval.request.text.clone());
                        monos.update(cx, |monos, cx| {
                            monos.allow_terminal(&approval.mono, kind, &text, cx)
                        });
                    }
                    self.mono_terminals_approved(
                        &approval.mono,
                        &approval.project,
                        approval.request.action,
                        &approval.request.input,
                        cx,
                    )
                }
            }
        };
        let _ = approval.reply.try_send(result);
        if self.mono_approvals.queue.is_empty() {
            if was_front {
                self.focus_active(window, cx);
            }
        } else {
            self.show_mono_approval(window, cx);
        }
        cx.notify();
    }

    /// Escape: declines the request in front.
    pub(crate) fn deny_front_mono_approval(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(seq) = self.mono_approvals.queue.front().map(|a| a.seq) else {
            return false;
        };
        self.answer_mono_approval(seq, Choice::Deny, window, cx);
        true
    }

    /// Chooses what button `ix` stands for. Run and Allow do nothing until the guard is over.
    fn press_mono_approval(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(front) = self.mono_approvals.queue.front() else {
            return;
        };
        let choice = match ix {
            DENY => Choice::Deny,
            _ if !front.armed => return,
            ALLOW => Choice::Allow,
            _ => Choice::Run,
        };
        let seq = front.seq;
        self.answer_mono_approval(seq, choice, window, cx);
    }

    fn mono_approval_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(front) = self.mono_approvals.queue.front_mut() else {
            return;
        };
        let keys = &event.keystroke;
        match keys.key.as_str() {
            "escape" => {
                self.deny_front_mono_approval(window, cx);
            }
            "enter" => {
                let ix = front.selected;
                self.press_mono_approval(ix, window, cx);
            }
            "right" | "left" => {
                let back = keys.key == "left";
                front.selected = (front.selected + if back { RUN } else { 1 }) % 3;
                cx.notify();
            }
            // Nothing else reaches the terminal or composer behind the dialog.
            _ => {}
        }
        cx.stop_propagation();
    }

    /// Tab and Shift-Tab move the focus between the buttons instead of leaving the dialog.
    fn move_mono_approval_focus(&mut self, back: bool, cx: &mut Context<Self>) {
        if let Some(front) = self.mono_approvals.queue.front_mut() {
            front.selected = (front.selected + if back { RUN } else { 1 }) % 3;
            cx.notify();
        }
    }

    /// The dialog, centred over the whole window above everything that blocks it.
    pub(super) fn mono_approval_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use crate::modal;
        let approval = self.mono_approvals.queue.front()?;
        let focus = self.mono_approvals.focus.as_ref()?;
        let request = &approval.request;
        let what = if request.kind == "command" {
            "run this command"
        } else {
            "type this text"
        };
        let allow = if request.kind == "command" {
            "Allow this command for this Mono"
        } else {
            "Allow this text for this Mono"
        };
        let more = self.mono_approvals.queue.len() - 1;
        let size = window.viewport_size();
        let width = 520f32.min(size.width.as_f32() - 24.);
        let line = |id: &'static str, name: &'static str, value: String| {
            div()
                .id(id)
                .flex()
                .gap_2()
                .text_size(px(12.))
                .child(
                    div()
                        .w(px(64.))
                        .flex_shrink_0()
                        .text_color(content(0.5))
                        .child(name),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_color(content(0.9))
                        .child(value),
                )
        };
        let bytes = request.text.len();
        let lines = request.text.matches('\n').count() + 1;
        let shown = visible(&request.text);
        let ring = |ix: usize| {
            if approval.selected == ix {
                content(0.55)
            } else {
                gpui::transparent_black()
            }
        };
        let guarded = |button: gpui::Stateful<gpui::Div>| modal::disabled(button, !approval.armed);
        let card = modal::card().w(px(width)).child(
            modal::card_body()
                .gap_3()
                .child(modal::title(format!(
                    "{} wants to {what}",
                    approval.mono_name
                )))
                .child(line(
                    "mono-approval-label",
                    "Terminal",
                    request.label.clone(),
                ))
                .child(line("mono-approval-cwd", "Folder", request.cwd.clone()))
                .when_some(request.then_key.clone(), |d, key| {
                    d.child(line("mono-approval-key", "Then", format!("presses {key}")))
                })
                .child(
                    div()
                        .id("mono-approval-text")
                        .debug_selector(|| "mono-approval-text".into())
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(content(0.1))
                        .bg(content(0.06))
                        .font_family(crate::fonts::mono())
                        .text_size(px(12.))
                        .line_height(px(17.))
                        .text_color(content(1.))
                        .child(shown),
                )
                .child(modal::body(format!(
                    "{bytes} bytes, {lines} line{}. Nothing runs until you choose Run.{}",
                    if lines == 1 { "" } else { "s" },
                    if more > 0 {
                        format!(" {more} more waiting.")
                    } else {
                        String::new()
                    }
                )))
                .child(
                    modal::actions()
                        .flex_wrap()
                        .child(
                            modal::ghost_button("mono-approval-deny", "Deny")
                                .debug_selector(|| "mono-approval-deny".into())
                                .border_1()
                                .border_color(ring(DENY))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.press_mono_approval(DENY, window, cx)
                                })),
                        )
                        .child(
                            guarded(
                                modal::ghost_button("mono-approval-allow", allow)
                                    .debug_selector(|| "mono-approval-allow".into())
                                    .border_1()
                                    .border_color(ring(ALLOW)),
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.press_mono_approval(ALLOW, window, cx),
                            )),
                        )
                        .child(
                            guarded(
                                modal::primary_button("mono-approval-run", "Run")
                                    .debug_selector(|| "mono-approval-run".into())
                                    .border_1()
                                    .border_color(ring(RUN)),
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.press_mono_approval(RUN, window, cx),
                            )),
                        ),
                ),
        );
        let priority = modal::BLOCKING_PRIORITY;
        Some(
            div()
                .absolute()
                .size_full()
                .child(modal::barrier("mono-approval-scrim", priority, window))
                .child(
                    deferred(
                        anchored().position(gpui::point(px(0.), px(0.))).child(
                            div()
                                .id("mono-approval")
                                .debug_selector(|| "mono-approval".into())
                                .role(Role::Dialog)
                                .aria_label("Approve a terminal command")
                                .track_focus(focus)
                                .key_context("MonoApproval")
                                .on_action(cx.listener(|this, _: &FocusNext, _, cx| {
                                    this.move_mono_approval_focus(false, cx)
                                }))
                                .on_action(cx.listener(|this, _: &FocusPrevious, _, cx| {
                                    this.move_mono_approval_focus(true, cx)
                                }))
                                .on_key_down(cx.listener(
                                    |this, event: &gpui::KeyDownEvent, window, cx| {
                                        this.mono_approval_key(event, window, cx)
                                    },
                                ))
                                .w(size.width)
                                .h(size.height)
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(modal::enter("mono-approval-card", card)),
                        ),
                    )
                    .with_priority(priority),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_character_a_person_could_not_see_is_written_out() {
        assert_eq!(visible("ls -la"), "ls -la");
        assert_eq!(visible("a\tb"), "a\\tb");
        assert_eq!(visible("one\ntwo"), "one\\n\ntwo");
        assert_eq!(visible("x\u{1b}[2Jy"), "x\\x1b[2Jy");
        assert_eq!(visible("a\u{7f}b\u{0}c\rd"), "a\\x7fb\\x00c\\x0dd");
        // C1 controls, zero-width and direction-changing characters, a no-break space.
        assert_eq!(visible("\u{9b}"), "\\u{9b}");
        assert_eq!(visible("a\u{200b}b"), "a\\u{200b}b");
        assert_eq!(visible("a\u{202e}b"), "a\\u{202e}b");
        assert_eq!(visible("a\u{a0}b"), "a\\u{a0}b");
        // Plain spaces and ordinary non-ASCII text are shown as they are.
        assert_eq!(visible("  café  "), "  café  ");
    }
}
