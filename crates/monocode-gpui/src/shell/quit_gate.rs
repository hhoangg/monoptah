//! Chrome-style hold-to-quit gate for the quit hotkey. One press only shows a
//! notice; the app quits when the key is held or pressed a second time. Menu
//! Quit, the tray and programmatic quits never come through here.
//!
//! macOS delivers no key-up for a letter while Command is down, so "still
//! held" is read from the key repeats the OS sends, which is why a press
//! carries whether it is a repeat. Cmd+Q is also a menu key equivalent, so the
//! press is taken by the window's key monitor (`shortcuts/macos.rs`) before
//! AppKit's menu or the keymap can quit. A menu click sends no key event and so
//! still quits directly. Elsewhere Ctrl+Q reaches `Shell::quit_hotkey_down`.
use super::*;
use crate::notifications::surface::glass_frame;
use gpui::{KeyDownEvent, Keystroke};
use std::time::{Duration, Instant};

/// How long the key must stay down for the hold to count as a quit.
pub(super) const QUIT_HOLD: Duration = Duration::from_millis(1000);
/// How long after a press a second press still counts as confirming it.
pub(super) const QUIT_ARM: Duration = Duration::from_millis(2000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuitGateStep {
    /// Show (or keep showing) the notice; the arm window restarted.
    Arm,
    Quit,
    Ignore,
}

#[derive(Clone, Copy)]
struct Armed {
    /// When the key first went down, which the hold is measured from.
    pressed_at: Instant,
    /// When the notice drops and the next press starts over.
    expires_at: Instant,
}

/// Pure gate state; the caller supplies the time.
#[derive(Default)]
pub(super) struct QuitGate(Option<Armed>);
impl QuitGate {
    pub(super) fn press(&mut self, repeat: bool, now: Instant) -> QuitGateStep {
        let Some(armed) = self.0.filter(|armed| now <= armed.expires_at) else {
            // A repeat with nothing armed is the tail of a press this window
            // never saw. Arming from it would start a hold already half spent.
            if repeat {
                self.0 = None;
                return QuitGateStep::Ignore;
            }
            self.0 = Some(Armed {
                pressed_at: now,
                expires_at: now + QUIT_ARM,
            });
            return QuitGateStep::Arm;
        };
        if !repeat || now.saturating_duration_since(armed.pressed_at) >= QUIT_HOLD {
            self.0 = None;
            return QuitGateStep::Quit;
        }
        self.0 = Some(Armed {
            expires_at: now + QUIT_ARM,
            ..armed
        });
        QuitGateStep::Arm
    }
    pub(super) fn expires_at(&self) -> Option<Instant> {
        self.0.map(|armed| armed.expires_at)
    }
    pub(super) fn reset(&mut self) {
        self.0 = None;
    }
}

/// Ctrl+Q off macOS (Cmd+Q on a Mac), however the keyboard cased the key.
fn is_quit_keystroke(keystroke: &Keystroke, mac: bool) -> bool {
    let m = keystroke.modifiers;
    keystroke.key.eq_ignore_ascii_case("q")
        && !m.alt
        && !m.shift
        && if mac {
            m.platform && !m.control
        } else {
            m.control && !m.platform
        }
}

/// `NSEventModifierFlag*` bits, so the monitor's decision is a function of
/// plain integers and can be tested anywhere.
const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_OPTION: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

/// Is this NSEvent key-down the quit hotkey? Command plus Q and nothing else;
/// Caps Lock only changes the reported case.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn is_quit_hotkey(modifiers: u64, characters: &str) -> bool {
    modifiers & FLAG_COMMAND != 0
        && modifiers & (FLAG_CONTROL | FLAG_OPTION | FLAG_SHIFT) == 0
        && characters.eq_ignore_ascii_case("q")
}

fn notice_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Hold ⌘Q to Quit"
    } else {
        "Hold Ctrl+Q to Quit"
    }
}

impl Shell {
    /// One quit hotkey press, key repeats included. Quitting goes through
    /// `request_quit`, which still asks about unsaved files.
    pub(super) fn quit_gate_press(
        &mut self,
        repeat: bool,
        now: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.quit_gate.press(repeat, now) {
            QuitGateStep::Ignore => {}
            QuitGateStep::Quit => {
                self.quit_gate_timer = None;
                cx.notify();
                self.request_quit(window, cx);
            }
            QuitGateStep::Arm => {
                let Some(deadline) = self.quit_gate.expires_at() else {
                    return;
                };
                // Replacing the task cancels the timer of the previous arm.
                self.quit_gate_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(deadline.saturating_duration_since(Instant::now()))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.quit_gate.expires_at() == Some(deadline) {
                            this.quit_gate.reset();
                            cx.notify();
                        }
                    });
                }));
                cx.notify();
            }
        }
    }

    /// Ctrl+Q off macOS. A key binding would quit directly and hides the key
    /// repeat flag, so the workspace root takes the key event instead. Ctrl+Q
    /// is XON in a terminal, which keeps it. On a Mac the window's key monitor
    /// owns Cmd+Q.
    pub(super) fn quit_hotkey_down(
        &mut self,
        event: &KeyDownEvent,
        mac: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mac || !is_quit_keystroke(&event.keystroke, false) {
            return;
        }
        if self.dock_focused(window, cx).is_some() {
            return;
        }
        cx.stop_propagation();
        self.quit_gate_press(event.is_held, Instant::now(), window, cx);
    }

    /// The centered pill, drawn above the toasts and out of the pointer's way.
    pub(super) fn quit_notice(&self) -> Option<AnyElement> {
        self.quit_gate.expires_at()?;
        let pill = glass_frame()
            .id("quit-gate-notice")
            .role(Role::Status)
            .debug_selector(|| "quit-gate-notice".into())
            .w_auto()
            .px(px(20.))
            .py(px(12.))
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .child(notice_label());
        let centered = div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(pill);
        Some(
            gpui::deferred(centered)
                .with_priority(120)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    const SHIFT: u64 = 1 << 17;
    const CONTROL: u64 = 1 << 18;
    const OPTION: u64 = 1 << 19;
    const COMMAND: u64 = 1 << 20;
    const CAPS: u64 = 1 << 16;

    fn stroke(modifiers: Modifiers, key: &str) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: None,
        }
    }
    fn control() -> Modifiers {
        Modifiers {
            control: true,
            ..Default::default()
        }
    }
    fn command() -> Modifiers {
        Modifiers {
            platform: true,
            ..Default::default()
        }
    }

    // Keystroke matcher (quitGate.test.ts isQuitHotkey)

    #[test]
    fn takes_command_q_on_a_mac_and_control_q_elsewhere() {
        assert!(is_quit_keystroke(&stroke(command(), "q"), true));
        assert!(is_quit_keystroke(&stroke(control(), "q"), false));
    }

    #[test]
    fn ignores_the_other_platforms_modifier() {
        assert!(!is_quit_keystroke(&stroke(control(), "q"), true));
        assert!(!is_quit_keystroke(&stroke(command(), "q"), false));
    }

    #[test]
    fn ignores_another_key_and_extra_modifiers() {
        assert!(!is_quit_keystroke(&stroke(command(), "w"), true));
        let shift = Modifiers {
            shift: true,
            ..command()
        };
        let alt = Modifiers {
            alt: true,
            ..command()
        };
        assert!(!is_quit_keystroke(&stroke(shift, "q"), true));
        assert!(!is_quit_keystroke(&stroke(alt, "q"), true));
    }

    #[test]
    fn accepts_the_key_however_the_keyboard_cased_it() {
        assert!(is_quit_keystroke(&stroke(command(), "Q"), true));
    }

    // Gate state machine (quitGate.test.ts quitGatePress)

    #[test]
    fn arms_on_the_first_press_instead_of_quitting() {
        let t = Instant::now();
        let mut gate = QuitGate::default();
        assert_eq!(gate.press(false, t), QuitGateStep::Arm);
        assert_eq!(gate.expires_at(), Some(t + QUIT_ARM));
    }

    #[test]
    fn quits_on_a_second_press_while_armed() {
        let t = Instant::now();
        let mut gate = QuitGate::default();
        gate.press(false, t);
        let step = gate.press(false, t + QUIT_ARM - Duration::from_millis(1));
        assert_eq!(step, QuitGateStep::Quit);
        assert_eq!(gate.expires_at(), None);
    }

    #[test]
    fn arms_again_rather_than_quitting_once_the_window_has_passed() {
        let t = Instant::now();
        let mut gate = QuitGate::default();
        gate.press(false, t);
        let late = t + QUIT_ARM + Duration::from_millis(1);
        assert_eq!(gate.press(false, late), QuitGateStep::Arm);
        assert_eq!(gate.expires_at(), Some(late + QUIT_ARM));
    }

    #[test]
    fn quits_once_a_repeat_says_the_key_is_still_down_past_the_hold() {
        let t = Instant::now();
        let mut gate = QuitGate::default();
        gate.press(false, t);
        assert_eq!(gate.press(true, t + QUIT_HOLD), QuitGateStep::Quit);
        assert_eq!(gate.expires_at(), None);
    }

    #[test]
    fn keeps_the_notice_up_without_quitting_on_an_early_repeat() {
        let t = Instant::now();
        let mut gate = QuitGate::default();
        gate.press(false, t);
        let early = t + QUIT_HOLD - Duration::from_millis(1);
        assert_eq!(gate.press(true, early), QuitGateStep::Arm);
        // Holding the key must not let the notice time out underneath it, and
        // the hold keeps measuring from the first press.
        assert_eq!(gate.expires_at(), Some(early + QUIT_ARM));
        assert_eq!(gate.press(true, t + QUIT_HOLD), QuitGateStep::Quit);
    }

    #[test]
    fn ignores_a_repeat_that_arrives_with_nothing_armed() {
        let mut gate = QuitGate::default();
        assert_eq!(gate.press(true, Instant::now()), QuitGateStep::Ignore);
        assert_eq!(gate.expires_at(), None);
    }

    // macOS monitor matcher (quit_hotkey.rs)

    #[test]
    fn plain_command_q_is_the_quit_hotkey() {
        assert!(is_quit_hotkey(COMMAND, "q"));
    }

    #[test]
    fn caps_only_changes_the_reported_case() {
        assert!(is_quit_hotkey(COMMAND | CAPS, "Q"));
    }

    #[test]
    fn other_modifiers_belong_to_other_shortcuts() {
        for extra in [SHIFT, CONTROL, OPTION] {
            assert!(!is_quit_hotkey(COMMAND | extra, "q"));
        }
    }

    #[test]
    fn command_is_required() {
        assert!(!is_quit_hotkey(0, "q"));
        assert!(!is_quit_hotkey(CONTROL, "q"));
    }

    #[test]
    fn only_the_q_key_is_gated() {
        for characters in ["", "w", "qq", "1"] {
            assert!(!is_quit_hotkey(COMMAND, characters), "{characters}");
        }
    }

    // Shell wiring

    fn ctrl_q(repeat: bool) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: stroke(control(), "q"),
            is_held: repeat,
            prefer_character_input: false,
        }
    }
    fn armed(window: gpui::WindowHandle<Shell>, cx: &mut VisualTestContext) -> bool {
        window
            .update(cx, |shell, _, _| shell.quit_gate.expires_at().is_some())
            .unwrap()
    }

    #[gpui::test]
    fn the_first_press_shows_a_centered_notice_until_the_window_expires(cx: &mut TestAppContext) {
        let (window, _, _) = super::super::tests::window(cx);
        cx.update(|cx| cx.set_reduce_motion(true));
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("quit-gate-notice").is_none());
        window
            .update(cx, |shell, w, cx| {
                shell.quit_gate_press(false, Instant::now(), w, cx)
            })
            .unwrap();
        cx.run_until_parked();
        let pill = cx.debug_bounds("quit-gate-notice").unwrap();
        let viewport = window.update(cx, |_, w, _| w.viewport_size()).unwrap();
        assert!((pill.center().x - viewport.width / 2.).abs() < px(2.));
        assert!((pill.center().y - viewport.height / 2.).abs() < px(2.));
        cx.executor()
            .advance_clock(QUIT_ARM + Duration::from_millis(1));
        cx.run_until_parked();
        assert!(cx.debug_bounds("quit-gate-notice").is_none());
        assert!(!armed(window, cx));
    }

    #[gpui::test]
    fn an_early_repeat_keeps_the_notice_past_the_first_deadline(cx: &mut TestAppContext) {
        let (window, _, _) = super::super::tests::window(cx);
        cx.update(|cx| cx.set_reduce_motion(true));
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let t = Instant::now();
        window
            .update(cx, |shell, w, cx| shell.quit_gate_press(false, t, w, cx))
            .unwrap();
        window
            .update(cx, |shell, w, cx| {
                shell.quit_gate_press(true, t + Duration::from_millis(500), w, cx)
            })
            .unwrap();
        // The first arm's timer is gone; only the extended one is left.
        cx.executor()
            .advance_clock(QUIT_ARM + Duration::from_millis(1));
        cx.run_until_parked();
        assert!(armed(window, cx));
        assert!(cx.debug_bounds("quit-gate-notice").is_some());
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        assert!(!armed(window, cx));
        assert!(cx.debug_bounds("quit-gate-notice").is_none());
    }

    #[gpui::test]
    fn ctrl_q_arms_the_gate_off_macos_only(cx: &mut TestAppContext) {
        let (window, _, _) = super::super::tests::window(cx);
        cx.update(|cx| cx.set_reduce_motion(true));
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let press = |cx: &mut VisualTestContext, event: KeyDownEvent, mac| {
            window
                .update(cx, |shell, w, cx| {
                    shell.quit_hotkey_down(&event, mac, w, cx)
                })
                .unwrap();
            armed(window, cx)
        };
        // Cmd+Q belongs to the window's key monitor on a Mac.
        assert!(!press(cx, ctrl_q(false), true));
        // A key repeat with nothing armed is the tail of an earlier press.
        assert!(!press(cx, ctrl_q(true), false));
        let other = KeyDownEvent {
            keystroke: stroke(control(), "w"),
            ..ctrl_q(false)
        };
        assert!(!press(cx, other, false));
        assert!(press(cx, ctrl_q(false), false));
    }

    #[gpui::test]
    fn ctrl_q_stays_a_terminal_key_while_the_dock_has_focus(cx: &mut TestAppContext) {
        let (window, _, b) = super::super::tests::window(cx);
        cx.update(|cx| {
            super::super::bind_keys(cx);
            crate::terminal::bind(cx);
            cx.set_reduce_motion(true);
        });
        window
            .update(cx, |shell, w, cx| {
                shell.open(b, w, cx);
                shell.focus_active(w, cx);
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-j");
        cx.run_until_parked();
        let focused = window
            .update(cx, |shell, w, cx| shell.dock_focused(w, cx).is_some())
            .unwrap();
        assert!(focused, "the dock should have the keys");
        window
            .update(cx, |shell, w, cx| {
                shell.quit_hotkey_down(&ctrl_q(false), false, w, cx)
            })
            .unwrap();
        assert!(!armed(window, cx));
    }
}
