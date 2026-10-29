//! What Escape acts on. It dismisses at most one thing per keystroke, and
//! only when nothing is left to dismiss does it stop the turn of the
//! conversation the user is looking at.
use super::*;

/// The conversation `Session: Stop` acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StopTarget {
    /// A Mono's own chat, as the open Mono or as a pane in a split.
    Mono(String),
    /// The session in the focused pane.
    Session(i64),
}

impl Shell {
    /// Close the topmost window-level overlay that the Workspace Escape binding
    /// shadows. These listen for Escape in `on_key_down`, which only runs when no
    /// action binding handled the key, so the shell closes them itself.
    pub(super) fn dismiss_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(view) = self.image_lightbox.as_ref().map(|(view, _)| view.clone()) {
            view.update(cx, |view, cx| view.close(window, cx));
            return true;
        }
        if self.notification_state.whats_new.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
            return true;
        }
        if self.resources.read(cx).is_open() {
            self.resources
                .update(cx, |monitor, cx| monitor.close(window, cx));
            return true;
        }
        if let Some(accounts) = crate::accounts::get(cx)
            && accounts.update(cx, |a, cx| a.dismiss_modal(window, cx))
        {
            return true;
        }
        if self.dismiss_background_dialog(window, cx) {
            return true;
        }
        if self.folder_picker.is_some() {
            self.close_folder_picker(window, cx);
            return true;
        }
        false
    }

    /// The conversation Escape would stop once nothing is left to close, or
    /// `None` when something else is in view or open. The focused pane is
    /// `workspace.active` (the title tab only names the tab holding a split),
    /// so a session that is merely selected in the sidebar, sits in an
    /// unfocused pane, or hides behind a Mono, Settings or an app view is not
    /// a target, and neither is a focused file or terminal pane.
    pub(super) fn escape_stop_target(&self, cx: &App) -> Option<StopTarget> {
        if self.menu.is_some()
            || self.settings.is_some()
            || self.app_search.is_some()
            || self.app_view_open()
            || self.session_preview.is_some()
            || crate::accounts::get(cx).is_some_and(|a| a.read(cx).has_modal_auth())
        {
            return None;
        }
        if let Some(id) = self.monos.as_ref().and_then(|m| m.read(cx).active.clone()) {
            return Some(StopTarget::Mono(id));
        }
        let pane = self.workspace.active?;
        if let Some(id) = self.mono_pane_id(pane) {
            return Some(StopTarget::Mono(id.to_owned()));
        }
        self.sessions
            .contains_key(&pane)
            .then_some(StopTarget::Session(pane))
    }

    /// Stop the target's turn; a target that is not busy is left alone, and a
    /// transient surface inside the conversation closes first.
    pub(super) fn stop_escape_target(
        &mut self,
        target: StopTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = match target {
            StopTarget::Mono(id) => self.monos.as_ref().and_then(|m| m.read(cx).open_chat(&id)),
            StopTarget::Session(id) => self.sessions.get(&id).cloned(),
        };
        if let Some(view) = view
            && !view.update(cx, |v, cx| v.dismiss_transient(window, cx))
        {
            view.update(cx, |v, cx| v.stop_if_busy(cx));
        }
    }
}
