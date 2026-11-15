//! The dialog that stands between a Mono and a shell: a free `terminals.open command` or
//! `terminals.send text` waits here until the user decides (see docs/agent-access.md).
//!
//! Rules this file keeps:
//! - Showing the dialog runs nothing. Only Run or Allow writes, and both re-run the call's
//!   checks first (`Shell::mono_terminals_approved`).
//! - One dialog at a time; later requests wait in a queue and each answers its own call. A
//!   Mono has at most one request waiting.
//! - Deny is the focused button and Escape denies. Until the dialog is armed, every key but
//!   Escape is ignored (and starts the `GUARD` wait over), and Run and Allow ignore clicks, so a
//!   keystroke or click aimed at what was on screen a moment ago cannot approve anything.
//!   The keyboard moves between Deny and Run only: "Allow this ... for this Mono" is a
//!   standing permission and is only ever given by a click.
//! - A request that outlives `APPROVAL_WAIT` (counted from the moment the bridge received the
//!   call), whose terminal, Mono or project went away, or whose Mono's turn was stopped, is
//!   answered with an error and dropped, and writes nothing even if answered late. The bridge
//!   waits longer than `APPROVAL_WAIT`, so a call is always answered while its Mono still
//!   waits, and one whose Mono stopped waiting (its channel closed) is dropped without running.
use super::mono_terminals::{MonoTerminalReply, Request, Via};
use super::*;
use gpui::{MouseButton, anchored, deferred};
use monocode_core::mono::{api::is_hidden_char, bridge::APPROVAL_WAIT};
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// How long the dialog ignores the user after it appears, and after each key pressed before
/// that (so a burst of typing aimed at what was on screen cannot outlast the guard).
pub(super) const GUARD: Duration = Duration::from_millis(1000);
/// How often the queue is checked against the terminals, Monos and projects it names.
const WATCH: Duration = Duration::from_millis(500);
pub(super) const DECLINED: &str = "The user declined this command";
const EXPIRED: &str =
    "The user did not answer in time, so nothing was run. Ask again only if it is still needed";
const GONE: &str = "This Mono or its project is no longer available";
const STOPPED: &str = "The Mono's turn was stopped, so nothing was run";
pub(super) const BUSY: &str =
    "Wait for the user's answer to your previous request before asking again";

/// The most lines the dialog's scroll box shows before the last line is also pinned below it,
/// and the most characters it shows on one line before the same.
const PIN_LINES: usize = 8;
const PIN_CHARS: usize = 400;

/// The buttons, left to right. The focused one starts on Deny. Only Deny and Run can be
/// focused: Allow is for the pointer.
const DENY: usize = 0;
const ALLOW: usize = 1;
const RUN: usize = 2;

pub(super) struct Approval {
    pub(super) seq: u64,
    pub(super) mono: String,
    pub(super) mono_name: String,
    pub(super) project: String,
    pub(super) request: Request,
    /// The Mono chat session that made the call, and when the bridge received it.
    pub(super) session: i64,
    pub(super) received: Instant,
    pub(super) reply: async_channel::Sender<Result<Value, String>>,
    /// The dialog has been put in front of the user (it is the first in the queue).
    pub(super) shown: bool,
    /// The dialog accepts input.
    pub(super) armed: bool,
    /// Counts the guard timers started for this dialog; only the latest one arms it.
    pub(super) guard: u64,
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
/// other control characters, and `\u{..}` for invisible, combining, direction-changing or
/// look-alike space characters. A literal backslash is shown doubled, so an escape written by
/// this function can never be mistaken for typed text, and a literal `·` is escaped because
/// `·` stands for a space where spaces would be hard to count (a run of two or more, and a
/// space at the start or end of a line).
pub(super) fn visible(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c == ' ' {
            let end = (at..chars.len())
                .find(|&i| chars[i] != ' ')
                .unwrap_or(chars.len());
            let edge = at == 0 || chars[at - 1] == '\n' || end == chars.len() || chars[end] == '\n';
            let marker = if end - at >= 2 || edge { '·' } else { ' ' };
            out.extend(std::iter::repeat_n(marker, end - at));
            at = end;
            continue;
        }
        at += 1;
        match c {
            '\n' => out.push_str("\\n\n"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '\u{b7}' => out.push_str("\\u{b7}"),
            c if c.is_ascii_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if is_hidden_char(c) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The last line of `text`, shown pinned below the scroll box when the text is long, so the
/// end of what runs is always in view. At most its last 160 characters.
fn pinned_last_line(text: &str) -> Option<String> {
    let lines = text.matches('\n').count() + 1;
    if lines <= PIN_LINES && text.chars().count() <= PIN_CHARS {
        return None;
    }
    let last = text.rsplit('\n').next().unwrap_or_default();
    if last.is_empty() {
        return Some("(ends with a line break)".into());
    }
    let count = last.chars().count();
    let tail: String = last.chars().skip(count.saturating_sub(160)).collect();
    Some(format!(
        "{}{}",
        if count > 160 { "..." } else { "" },
        visible(&tail)
    ))
}

impl Shell {
    pub(super) fn queue_mono_approval(
        &mut self,
        mono: &str,
        project: &str,
        request: Request,
        via: Via,
        cx: &mut Context<Self>,
    ) -> MonoTerminalReply {
        // One request at a time per Mono: a second one cannot pile up behind the first and
        // be approved by a click meant for something else.
        if self.mono_approvals.queue.iter().any(|a| a.mono == mono) {
            return MonoTerminalReply::Now(Err(BUSY.into()));
        }
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
            session: via.session,
            received: via.received,
            reply,
            shown: false,
            armed: false,
            guard: 0,
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
            front.selected = DENY;
            self.restart_mono_guard(cx);
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

    /// Disarms the front dialog and starts the guard over. A dialog that just appeared, or that
    /// the user is still typing at, takes no input until a whole `GUARD` has passed quietly.
    fn restart_mono_guard(&mut self, cx: &mut Context<Self>) {
        let Some(front) = self.mono_approvals.queue.front_mut() else {
            return;
        };
        front.armed = false;
        front.guard += 1;
        let (seq, epoch) = (front.seq, front.guard);
        let guard = cx.background_executor().timer(GUARD);
        cx.spawn(async move |this, cx| {
            guard.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(a) = this
                    .mono_approvals
                    .queue
                    .iter_mut()
                    .find(|a| a.seq == seq && a.guard == epoch)
                {
                    a.armed = true;
                    cx.notify();
                }
            });
        })
        .detach();
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
        // The Mono stopped waiting for this answer: its turn was stopped or ended.
        if self.monos.as_ref().is_some_and(|monos| {
            monos
                .read(cx)
                .session_stopped(approval.session, &approval.mono, cx)
        }) {
            return Some(STOPPED.into());
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
                } else if approval.received.elapsed() >= APPROVAL_WAIT {
                    // The Mono's wait is over or nearly: a click now must not run anything.
                    Err(EXPIRED.to_owned())
                } else if let Some(problem) = self.approval_problem(&approval, cx) {
                    Err(problem)
                } else {
                    let result = self.mono_terminals_approved(
                        &approval.mono,
                        &approval.project,
                        &approval.request,
                        cx,
                    );
                    // Remembered only once the checked write went through.
                    if result.is_ok()
                        && choice == Choice::Allow
                        && let Some(monos) = self.monos.clone()
                    {
                        let (kind, text) = (approval.request.kind, approval.request.text.clone());
                        monos.update(cx, |monos, cx| {
                            monos.allow_terminal(&approval.mono, &approval.project, kind, &text, cx)
                        });
                    }
                    result
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

    /// Chooses what button `ix` stands for. Run and Allow do nothing until the dialog is armed.
    /// `by_key` is Enter: it never gives Allow, a standing permission that only a click gives.
    fn press_mono_approval(
        &mut self,
        ix: usize,
        by_key: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(front) = self.mono_approvals.queue.front() else {
            return;
        };
        let choice = match ix {
            DENY => Choice::Deny,
            _ if !front.armed => return,
            ALLOW if by_key => return,
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
        let Some(front) = self.mono_approvals.queue.front() else {
            return;
        };
        let keys = &event.keystroke;
        if keys.key == "escape" {
            self.deny_front_mono_approval(window, cx);
        } else if !front.armed {
            // Nothing but Escape reaches the dialog before it is armed (Tab and the arrows
            // included), and each key starts the wait over.
            self.restart_mono_guard(cx);
        } else {
            match keys.key.as_str() {
                "enter" => {
                    let ix = front.selected;
                    self.press_mono_approval(ix, true, window, cx);
                }
                "right" | "left" => self.move_mono_approval_focus(keys.key == "left", cx),
                // Nothing else reaches the terminal or composer behind the dialog.
                _ => {}
            }
        }
        cx.stop_propagation();
    }

    /// Tab, Shift-Tab and the arrows move between Deny and Run, once the dialog is armed. A
    /// key before that only starts the guard over.
    fn move_mono_approval_focus(&mut self, _back: bool, cx: &mut Context<Self>) {
        let Some(front) = self.mono_approvals.queue.front_mut() else {
            return;
        };
        if !front.armed {
            self.restart_mono_guard(cx);
            return;
        }
        front.selected = if front.selected == DENY { RUN } else { DENY };
        cx.notify();
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
        // Typing text at a shell is running it once Enter follows, so the dialog says so.
        let what = if request.kind == "command" {
            "run this command"
        } else if request.then_key.as_deref() == Some("enter") {
            "run this in a terminal"
        } else {
            "type this into a terminal, where it can run"
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
                .debug_selector(move || id.into())
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
        let escaped = shown != request.text;
        let pinned = pinned_last_line(&request.text);
        let non_ascii = !request.text.is_ascii();
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
                    visible(&request.label),
                ))
                .child(line(
                    "mono-approval-program",
                    "Program",
                    match &request.seen {
                        Some(seen) => seen.front.name().to_owned(),
                        None => "a new shell".to_owned(),
                    },
                ))
                .child(line(
                    "mono-approval-cwd",
                    // Where the terminal is now when the system says; else where it started,
                    // which a `cd` may have changed.
                    if request.cwd_live { "Folder" } else { "Started in" },
                    visible(&request.cwd),
                ))
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
                .when_some(pinned, |d, last| {
                    d.child(
                        div()
                            .id("mono-approval-last")
                            .debug_selector(|| "mono-approval-last".into())
                            .flex()
                            .gap_2()
                            .text_size(px(12.))
                            .child(div().w(px(64.)).flex_shrink_0().text_color(content(0.5)).child("Last line"))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .font_family(crate::fonts::mono())
                                    .text_color(content(1.))
                                    .child(last),
                            ),
                    )
                })
                .child(modal::body(format!(
                    "{bytes} bytes, {lines} line{}. Run {}. Nothing runs until you choose Run.{}",
                    if lines == 1 { "" } else { "s" },
                    match (request.kind, request.then_key.as_deref()) {
                        ("command", _) => "types this and presses Enter, so it runs".to_owned(),
                        (_, Some(key)) => format!("types this and presses {key}, so it runs"),
                        _ => "types this at the terminal's current line, after whatever is already there"
                            .to_owned(),
                    },
                    if more > 0 {
                        format!(" {more} more waiting.")
                    } else {
                        String::new()
                    }
                )))
                .when(escaped, |d| {
                    d.child(modal::body(
                        "\\n is a line break, \\\\ a backslash, a dot a space that is hard to see.",
                    ))
                })
                .when(non_ascii, |d| {
                    d.child(
                        div()
                            .id("mono-approval-non-ascii")
                            .debug_selector(|| "mono-approval-non-ascii".into())
                            .text_size(px(12.))
                            .text_color(content(0.9))
                            .child(
                                "Contains non-ASCII characters. \\u{..} marks ones you cannot see; check the rest for look-alikes.",
                            ),
                    )
                })
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
                                    this.press_mono_approval(DENY, false, window, cx)
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
                                |this, _, window, cx| this.press_mono_approval(ALLOW, false, window, cx),
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
                                |this, _, window, cx| this.press_mono_approval(RUN, false, window, cx),
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
        // Ordinary non-ASCII text is shown as it is, a single space too.
        assert_eq!(
            visible("caf\u{e9} \u{65e5}\u{672c}"),
            "caf\u{e9} \u{65e5}\u{672c}"
        );
    }

    #[test]
    fn characters_that_draw_nothing_or_look_like_something_else_are_escaped() {
        for (c, shown) in [
            ('\u{3164}', "\\u{3164}"),
            ('\u{115f}', "\\u{115f}"),
            ('\u{1160}', "\\u{1160}"),
            ('\u{ffa0}', "\\u{ffa0}"),
            ('\u{2800}', "\\u{2800}"),
            ('\u{fe0f}', "\\u{fe0f}"),
            ('\u{e0041}', "\\u{e0041}"),
            ('\u{301}', "\\u{301}"),
            ('\u{2003}', "\\u{2003}"),
            ('\u{3000}', "\\u{3000}"),
            ('\u{2066}', "\\u{2066}"),
            ('\u{feff}', "\\u{feff}"),
        ] {
            assert_eq!(visible(&format!("a{c}b")), format!("a{shown}b"), "{shown}");
        }
    }

    #[test]
    fn an_escape_is_never_confused_with_typed_text() {
        // A real tab and the two characters backslash and t look different.
        assert_ne!(visible("a\tb"), visible("a\\tb"));
        assert_eq!(visible("a\\tb"), "a\\\\tb");
        assert_eq!(visible("a\\nb"), "a\\\\nb");
        assert_ne!(visible("a\nb"), visible("a\\nb"));
        // The middle dot that stands for a space is escaped when it is typed.
        assert_eq!(visible("a\u{b7}b"), "a\\u{b7}b");
    }

    #[test]
    fn runs_of_spaces_and_spaces_at_a_line_edge_can_be_counted() {
        assert_eq!(visible("a b"), "a b");
        assert_eq!(visible("a  b"), "a\u{b7}\u{b7}b");
        assert_eq!(visible("a    b"), "a\u{b7}\u{b7}\u{b7}\u{b7}b");
        assert_eq!(visible(" a"), "\u{b7}a");
        assert_eq!(visible("a "), "a\u{b7}");
        assert_eq!(visible("a \nb"), "a\u{b7}\\n\nb");
        assert_eq!(visible("a\n b"), "a\\n\n\u{b7}b");
        assert_eq!(
            visible("  caf\u{e9}  "),
            "\u{b7}\u{b7}caf\u{e9}\u{b7}\u{b7}"
        );
    }

    #[test]
    fn the_last_line_is_pinned_only_for_long_text() {
        assert_eq!(pinned_last_line("ls -la"), None);
        assert_eq!(pinned_last_line(&"x\n".repeat(7)), None);
        let long = format!("{}\nrm -rf build", "echo ok\n".repeat(20));
        assert_eq!(pinned_last_line(&long).as_deref(), Some("rm -rf build"));
        assert_eq!(
            pinned_last_line(&format!("{}\n", "echo ok\n".repeat(20))).as_deref(),
            Some("(ends with a line break)")
        );
        // A long single line pins its end, with the spaces made countable.
        let wide = format!("{}  && rm x", "a".repeat(500));
        let pinned = pinned_last_line(&wide).unwrap();
        assert!(
            pinned.starts_with("...") && pinned.ends_with("··&& rm x"),
            "{pinned}"
        );
    }
}
