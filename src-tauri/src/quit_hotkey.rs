//! Chrome-style "Hold ⌘Q to Quit" gate for macOS.
//!
//! The Quit menu item keeps its `CmdOrCtrl+Q` accelerator, so the File/app menu
//! still shows the ⌘Q badge users look for. What changes is who gets the key
//! press first: an app-wide local key-down monitor sees it, forwards it to the
//! focused webview, and swallows it. Swallowing is the whole point — an event
//! the monitor returns reaches `NSMenu`, which fires the accelerator and quits
//! instantly, which is exactly the behaviour the gate replaces.
//!
//! The monitor exists because a menu key equivalent cannot measure a hold.
//! A key equivalent does not auto-repeat, and AppKit delivers no `keyUp` for a
//! letter while Command is held, so neither the menu nor the webview can tell
//! "pressed and released" from "still down". A raw key-down monitor does get
//! the auto-repeats, so it forwards each one with `isARepeat` and lets the
//! frontend time the hold from them.
//!
//! Dropping the accelerator instead would have worked for the key press and
//! lost the ⌘Q hint in the menu, which is why the monitor swallows rather than
//! the menu giving the shortcut up.

use tauri::AppHandle;

/// Sent to the focused window for every ⌘Q key-down, auto-repeats included;
/// the repeats are what let the frontend see that the key is still held.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const QUIT_GATE_PRESS: &str = "quit_gate_press";

/// `NSEventModifierFlag*` bit positions, spelled out so the decision below
/// stays a function of plain integers and can be tested off macOS.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const FLAG_SHIFT: u64 = 1 << 17;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const FLAG_CONTROL: u64 = 1 << 18;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const FLAG_OPTION: u64 = 1 << 19;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const FLAG_COMMAND: u64 = 1 << 20;

#[cfg(target_os = "macos")]
#[derive(Clone, serde::Serialize)]
struct QuitGatePress {
    repeat: bool,
}

/// Is this key-down the quit hotkey? Command plus Q and nothing else: control,
/// option or shift make it somebody else's shortcut, so the gate must let those
/// through untouched. Caps lock is not in the reject set because it only
/// changes the case AppKit reports for the character.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_quit_hotkey(modifiers: u64, characters: &str) -> bool {
    modifiers & FLAG_COMMAND != 0
        && modifiers & (FLAG_CONTROL | FLAG_OPTION | FLAG_SHIFT) == 0
        && characters.eq_ignore_ascii_case("q")
}

#[cfg(target_os = "macos")]
thread_local! {
    /// AppKit returns a token for the installed monitor. Parking it here keeps
    /// it alive for the life of the app and makes a second `install` a no-op
    /// instead of stacking two monitors that both swallow the same press.
    static MONITOR: std::cell::RefCell<Option<objc2::rc::Retained<objc2::runtime::AnyObject>>> =
        const { std::cell::RefCell::new(None) };
}

/// Put the gate in front of the menu accelerator. Safe to call more than once.
#[cfg(target_os = "macos")]
pub fn install(app: &AppHandle) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSEventMask};
    use std::ptr::NonNull;

    // Both the monitor registry and the window lookups the handler does are
    // main-thread only.
    if MainThreadMarker::new().is_none() {
        return;
    }
    if MONITOR.with(|slot| slot.borrow().is_some()) {
        return;
    }

    let app = app.clone();
    let handler = block2::RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // Returning this pointer hands the event back to normal handling.
        let unchanged = event.as_ptr();
        // The mask admits key-downs only, so `characters` and `isARepeat` are
        // both answerable here.
        let event = unsafe { event.as_ref() };
        let characters = event.characters().map(|text| text.to_string());
        let modifiers = event.modifierFlags().0 as u64;
        if !is_quit_hotkey(modifiers, characters.as_deref().unwrap_or_default()) {
            return unchanged;
        }
        let Some(label) = gate_window(&app) else {
            // Nothing can draw the overlay, so leave ⌘Q to the menu rather
            // than swallow it into a shortcut that does nothing at all.
            return unchanged;
        };
        let press = QuitGatePress {
            repeat: event.isARepeat(),
        };
        if tauri::Emitter::emit_to(&app, &label, QUIT_GATE_PRESS, press).is_err() {
            return unchanged;
        }
        std::ptr::null_mut()
    });

    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
    };
    // Fail soft: with no monitor the menu accelerator still quits, only without
    // the hold gate in front of it.
    MONITOR.with(|slot| *slot.borrow_mut() = monitor);
}

/// Which window hosts the overlay. Same fallback the menu uses for its
/// single-window items: the focused window, else any visible one, else any —
/// the gate should cover the press even when focus sits on a panel that has no
/// overlay of its own.
#[cfg(target_os = "macos")]
fn gate_window(app: &AppHandle) -> Option<String> {
    let windows = crate::window::workspace_windows(app);
    windows
        .iter()
        .find(|window| window.is_focused().unwrap_or(false))
        .or_else(|| {
            windows
                .iter()
                .find(|window| window.is_visible().unwrap_or(false))
        })
        .or(windows.first())
        .map(|window| window.label().to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &AppHandle) {}

/// The overlay's verdict: ⌘Q was held long enough. Goes through the same
/// request as the menu item, so the busy-session poll and the one confirmation
/// dialog still happen here and only here.
#[tauri::command]
pub fn quit_now(app: AppHandle) {
    crate::window::request_quit(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_command_q_is_the_quit_hotkey() {
        assert!(is_quit_hotkey(FLAG_COMMAND, "q"));
    }

    #[test]
    fn caps_only_changes_the_reported_case() {
        assert!(is_quit_hotkey(FLAG_COMMAND | (1 << 16), "Q"));
    }

    #[test]
    fn other_modifiers_belong_to_other_shortcuts() {
        for extra in [FLAG_SHIFT, FLAG_CONTROL, FLAG_OPTION] {
            assert!(!is_quit_hotkey(FLAG_COMMAND | extra, "q"));
        }
    }

    #[test]
    fn command_is_required() {
        assert!(!is_quit_hotkey(0, "q"));
        assert!(!is_quit_hotkey(FLAG_CONTROL, "q"));
    }

    #[test]
    fn only_the_q_key_is_gated() {
        for characters in ["", "w", "qq", "1"] {
            assert!(!is_quit_hotkey(FLAG_COMMAND, characters), "{characters}");
        }
    }
}
