//! Workspace fallback for orphaned GPUI focus and embedded AppKit previews.
//! Normal focused inputs keep the keymap's context ordering. Resolve from the
//! live keymap, including user overrides, rather than maintaining another list
//! of physical shortcuts.
use super::*;
use gpui::{Action, KeyContext, Keystroke, WeakEntity};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
mod macos;

#[derive(Default)]
struct Forwarding {
    native: Cell<bool>,
    handled: Cell<bool>,
    chord_deadline: Cell<Option<Instant>>,
}
pub(super) struct Shortcuts {
    _keys: Subscription,
    #[cfg(target_os = "macos")]
    _native: Option<macos::NativeKeys>,
}

/// Actions of the app and the window itself, which keep working behind a blocking dialog:
/// from the menu bar and from their key bindings (Hide, Minimize, Quit...).
fn app_level_action(action: &dyn Action) -> bool {
    matches!(
        action.name(),
        "application::Hide"
            | "application::HideOthers"
            | "application::ShowAll"
            | "application::Quit"
            | "application::OpenUrl"
            | "workspace::MinimizeWindow"
            | "workspace::ZoomWindow"
            | "workspace::About"
    )
}

/// Pure modifier presses are observations, not strokes of a shortcut.
fn is_modifier_key(keystroke: &Keystroke) -> bool {
    matches!(
        keystroke.key.as_str(),
        "control" | "platform" | "alt" | "shift" | "function"
    )
}

/// While a blocking dialog is open the menu bar must not reach the workspace behind it: a
/// menu action dispatches to the focused node, which sits inside the Shell. Registers a
/// capture listener that swallows each workspace-level menu action; the app and window level
/// ones (see [`app_level_action`]) are left out on purpose.
pub(super) fn block_menu_actions<E: InteractiveElement>(element: E) -> E {
    macro_rules! swallow {
        ($el:expr; $($action:ty),+ $(,)?) => {{
            let mut el = $el;
            $(el = el.capture_action(|_: &$action, _, cx| cx.stop_propagation());)+
            el
        }};
    }
    swallow!(
        element;
        NewCodex, NewClaude, OpenProject, CloseTab, CloseAllTabs, CloseOtherTabs, VisitBack,
        VisitForward, NextTab, PreviousTab, ToggleSidebar, FocusSearch, SearchFiles,
        PreviousSession, NextSession, PreviousProject, NextProject, SwitchModel,
        ToggleRightPanel, SplitRight, SplitDown, FocusPaneLeft, FocusPaneRight, FocusPaneUp,
        FocusPaneDown, ToggleTerminal, NewTerminal, NewTerminalHome, SplitTerminal,
        ToggleLayout, ToggleWorkspaceStyle, OpenSettings, GoToFile, OpenCommandPalette,
        OpenInbox, OpenNotes, CheckForUpdates, ZoomIn, ZoomOut, ResetZoom, Reload,
        SidebarAppearance, crate::file_editor::ToggleAutosave,
        crate::file_editor::ToggleGitBlame,
    )
}

fn workspace_action(action: &dyn Action) -> bool {
    let name = action.name();
    (name.starts_with("workspace::")
        && !matches!(
            name,
            "workspace::FocusNext" | "workspace::FocusPrevious" | "workspace::DismissMenu"
        ))
        || name == "application::Quit"
        || name == "file_editor::ToggleGitBlame"
}

/// Keymap owns precedence, disabled bindings, physical-key matching and chords.
fn candidates(keys: &[Keystroke], cx: &App) -> Vec<KeyBinding> {
    let context = KeyContext::parse("Workspace").expect("static workspace key context");
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let contexts = [context];
    let (complete, _) = keymap.bindings_for_input(keys, &contexts);
    complete
        .into_iter()
        .chain(keymap.possible_next_bindings_for_input(keys, &contexts))
        .filter(|binding| workspace_action(binding.action()))
        .collect()
}

/// What the key monitor calls for every Cmd+Q key-down, repeats included.
/// Returns false when no Shell can show the notice, so the press goes on to
/// quit as it always did instead of vanishing.
#[cfg(target_os = "macos")]
fn quit_press(
    owner: &WeakEntity<Shell>,
    window: &Window,
    cx: &App,
) -> Rc<dyn Fn(bool, Instant) -> bool> {
    let owner = owner.clone();
    let handle = window.window_handle().downcast::<Shell>();
    let app = cx.to_async();
    Rc::new(move |repeat, now| {
        let (Some(handle), true) = (handle, owner.upgrade().is_some()) else {
            return false;
        };
        // The monitor runs inside AppKit's event dispatch, so the update waits
        // for the next turn rather than re-entering the app.
        let mut cx = app.clone();
        app.foreground_executor()
            .spawn(async move {
                let _ = handle.update(&mut cx, |shell, window, cx| {
                    shell.quit_gate_press(repeat, now, window, cx)
                });
            })
            .detach();
        true
    })
}

impl Shell {
    /// The focus handle of the open blocking dialog (continue locally, or archive a cloud
    /// session), which must own the keyboard and pointer until it closes.
    pub(super) fn blocking_modal(&self) -> Option<FocusHandle> {
        self.cloud_continue
            .dialog
            .as_ref()
            .map(|d| d.focus_handle())
            .or_else(|| self.cloud_archive.as_ref().map(|d| d.focus_handle()))
    }

    /// Escape and Enter for the open blocking dialog. Every other key is ignored.
    fn blocking_modal_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let escape = keystroke.key == "escape";
        let enter = keystroke.key == "enter";
        if self.cloud_continue.dialog.is_some() {
            if escape {
                self.close_continue_locally(window, cx);
            }
        } else if let Some(dialog) = self.cloud_archive.as_ref() {
            if escape {
                self.dismiss_cloud_archive(window, cx);
            } else if enter && matches!(dialog.phase, cloud::Phase::Choose) {
                let default = cloud::default_choice(dialog.others());
                self.confirm_cloud_archive(default, window, cx);
            }
        }
    }
}

pub(super) fn install(owner: WeakEntity<Shell>, window: &Window, cx: &mut App) -> Shortcuts {
    let window_id = window.window_handle().window_id();
    let forwarding = Rc::new(Forwarding::default());
    #[cfg(target_os = "macos")]
    let native = macos::NativeKeys::new(window, forwarding.clone(), quit_press(&owner, window, cx));
    let pending = RefCell::new(Vec::<Keystroke>::new());
    let keys = cx.intercept_keystrokes(move |event, window, cx| {
        if window.window_handle().window_id() != window_id {
            return;
        }
        let Some(owner) = owner.upgrade() else {
            return;
        };
        // A blocking dialog's focus sits inside the Workspace key context, so without this
        // every workspace binding (and Tab focus moves) would act behind the dialog.
        if let Some(focus) = owner.read(cx).blocking_modal() {
            pending.borrow_mut().clear();
            forwarding.chord_deadline.set(None);
            if super::quit_gate::is_quit_keystroke(&event.keystroke, cfg!(target_os = "macos")) {
                return;
            }
            if is_modifier_key(&event.keystroke) {
                return;
            }
            // Hide, Minimize and the like are app-level, not workspace behind the dialog.
            let bound = {
                let keymap = cx.key_bindings();
                let keymap = keymap.borrow();
                let (bindings, _) = keymap.bindings_for_input(
                    std::slice::from_ref(&event.keystroke),
                    &event.context_stack,
                );
                bindings
                    .first()
                    .is_some_and(|binding| app_level_action(binding.action()))
            };
            if bound {
                return;
            }
            if !focus.contains_focused(window, cx) {
                window.focus(&focus, cx);
            }
            owner.update(cx, |shell, cx| {
                shell.blocking_modal_key(&event.keystroke, window, cx)
            });
            forwarding.handled.set(true);
            cx.stop_propagation();
            return;
        }
        let root = owner.read(cx).focus.clone();
        let native = forwarding.native.get();
        let now = Instant::now();
        if forwarding
            .chord_deadline
            .get()
            .is_some_and(|until| now >= until)
        {
            pending.borrow_mut().clear();
            forwarding.chord_deadline.set(None);
        }
        if !native && root.contains_focused(window, cx) && pending.borrow().is_empty() {
            return;
        }
        // Modifier release observations are not strokes in a shortcut chord.
        if is_modifier_key(&event.keystroke) {
            return;
        }
        let mut sequence = pending.borrow_mut();
        sequence.push(event.keystroke.clone());
        let mut matches = candidates(&sequence, cx);
        if matches.is_empty() && sequence.len() > 1 {
            sequence.clear();
            sequence.push(event.keystroke.clone());
            matches = candidates(&sequence, cx);
        }
        let action = matches
            .iter()
            .find(|binding| binding.match_keystrokes(sequence.as_slice()) == Some(false))
            .map(|binding| binding.action().boxed_clone());
        let partial = action.is_none() && !matches.is_empty();
        if !partial {
            sequence.clear();
        }
        drop(sequence);
        forwarding
            .chord_deadline
            .set(partial.then(|| now + Duration::from_secs(1)));
        if let Some(action) = action {
            if !native && !root.contains_focused(window, cx) {
                // Repair lost GPUI focus without disturbing an embedded native
                // view's selection or its first-responder ownership.
                owner.update(cx, |shell, cx| shell.focus_workspace(window, cx));
            }
            root.dispatch_action(action.as_ref(), window, cx);
            forwarding.handled.set(true);
            cx.stop_propagation();
        } else if partial {
            forwarding.handled.set(true);
            cx.stop_propagation();
        } else if native {
            // The monitor re-delivers the original event to AppKit. Prevent
            // the stale GPUI input from processing Copy, typing, IME, etc.
            cx.stop_propagation();
        }
    });
    Shortcuts {
        _keys: keys,
        #[cfg(target_os = "macos")]
        _native: native,
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    #[gpui::test]
    fn lost_focus_shortcuts_follow_live_remaps_and_disabled_bindings(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::input::bind(cx);
            super::super::bind_keys(cx);
            cx.set_reduce_motion(true);
        });
        let (window, _, _) = super::super::tests::window(cx);
        cx.refresh().unwrap();
        let orphan = window
            .update(cx, |_, window, cx| {
                let orphan = cx.focus_handle();
                window.focus(&orphan, cx);
                orphan
            })
            .unwrap();
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.simulate_keystrokes(window.into(), &format!("{modifier}-b"));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| assert!(!shell.workspace.sidebar_visible))
            .unwrap();
        for (binding, stroke, expected) in [
            (None, format!("{modifier}-b"), false),
            (
                Some(format!("{modifier}-alt-b")),
                format!("{modifier}-b"),
                false,
            ),
            (
                Some(format!("{modifier}-alt-b")),
                format!("{modifier}-alt-b"),
                true,
            ),
            (
                Some(format!("{modifier}-alt-y {modifier}-alt-b")),
                format!("{modifier}-alt-y {modifier}-alt-b"),
                false,
            ),
        ] {
            window
                .update(cx, |shell, window, cx| {
                    shell
                        .workspace
                        .preferences
                        .keybindings
                        .insert("App: Toggle Sidebar".into(), binding);
                    crate::settings::apply_keybindings(&shell.workspace.preferences, cx);
                    window.focus(&orphan, cx);
                })
                .unwrap();
            cx.simulate_keystrokes(window.into(), &stroke);
            cx.run_until_parked();
            window
                .update(cx, |shell, _, _| {
                    assert_eq!(shell.workspace.sidebar_visible, expected)
                })
                .unwrap();
        }
    }
    #[gpui::test]
    fn toggle_git_blame_flips_the_column_from_the_shortcut_and_the_menu_action(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            crate::input::bind(cx);
            super::super::bind_keys(cx);
            crate::file_editor::bind(cx);
            cx.set_reduce_motion(true);
        });
        let (window, _, _) = super::super::tests::window(cx);
        cx.refresh().unwrap();
        let column = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, _| {
                    shell.workspace.preferences.editor_blame_column
                })
                .unwrap()
        };
        assert!(!column(cx), "off by default");
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.simulate_keystrokes(window.into(), &format!("{modifier}-alt-b"));
        cx.run_until_parked();
        assert!(column(cx));
        // The menu item dispatches the same action.
        window
            .update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(crate::file_editor::ToggleGitBlame), cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(!column(cx));
        // And the shortcut still works once focus is lost.
        window
            .update(cx, |_, window, cx| {
                let orphan = cx.focus_handle();
                window.focus(&orphan, cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), &format!("{modifier}-alt-b"));
        cx.run_until_parked();
        assert!(column(cx));
    }
}
