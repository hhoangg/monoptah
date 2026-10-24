//! Terminal sessions in the shell: a session whose surface is the terminal gets
//! a `TuiSession` here and never a `Conversation`. Each one launches when first
//! shown and ends when its pane leaves the workspace (tab or pane closed,
//! archived, deleted).
use super::*;
use crate::tui_session::{TuiEvent, TuiSession, runs_in_terminal};

#[cfg(test)]
mod tests;

/// `quitWhileBusyMessage(0, terminals)`: a terminal turn dies with its CLI and is
/// never re-sent, but its conversation is kept. Native has no chat-turn quit
/// confirmation, so the message names only terminals.
pub(super) fn quit_while_busy_message(terminals: usize) -> String {
    let turns = format!(
        "{terminals} terminal {}",
        if terminals == 1 { "turn" } else { "turns" }
    );
    if terminals == 1 {
        format!(
            "{turns} is still running. Quit anyway? It will stop, though its conversation is saved."
        )
    } else {
        format!(
            "{turns} are still running. Quit anyway? They will stop, though their conversations are saved."
        )
    }
}

/// What the one quit dialog says.
pub(super) struct QuitPrompt {
    pub message: String,
    pub detail: Option<String>,
    pub confirm: &'static str,
}

/// The dialog for what quitting would lose, or none when nothing would be. Unsaved
/// file edits and busy terminals share one dialog rather than two in a row.
pub(super) fn quit_prompt(busy_terminals: usize, unsaved_files: bool) -> Option<QuitPrompt> {
    match (busy_terminals, unsaved_files) {
        (0, false) => None,
        (0, true) => Some(QuitPrompt {
            message: "Quit with unsaved file edits?".into(),
            detail: Some("Save your files before quitting to keep your changes.".into()),
            confirm: "Quit Without Saving",
        }),
        (terminals, unsaved) => Some(QuitPrompt {
            message: quit_while_busy_message(terminals),
            detail: unsaved.then(|| {
                "Unsaved file edits will also be lost. Save your files first to keep them.".into()
            }),
            confirm: if unsaved {
                "Quit Without Saving"
            } else {
                "Quit"
            },
        }),
    }
}

impl Shell {
    /// The summary of a session that runs as its provider's CLI.
    pub(super) fn terminal_summary(&self, id: i64) -> Option<&Session> {
        self.session_summaries
            .get(&id)
            .filter(|summary| runs_in_terminal(summary))
    }
    pub(super) fn is_terminal(&self, id: i64) -> bool {
        self.terminal_summary(id).is_some()
    }
    /// A running terminal session's CLI is mid-turn (Claude's hooks say so).
    pub(super) fn terminal_busy(&self, id: i64, cx: &App) -> bool {
        self.tui_sessions
            .get(&id)
            .is_some_and(|tui| tui.read(cx).busy())
    }

    /// Terminal turns running now, which quitting would stop.
    pub(super) fn busy_terminals(&self, cx: &App) -> usize {
        self.tui_sessions
            .values()
            .filter(|tui| tui.read(cx).busy())
            .count()
    }
    /// The dialog quitting needs, if anything would be lost.
    pub(super) fn quit_prompt(&self, cx: &App) -> Option<QuitPrompt> {
        quit_prompt(self.busy_terminals(cx), self.unsaved_files(cx))
    }
    /// Closing the window ends the app, so it asks what quitting asks.
    pub(super) fn close_needs_confirmation(&self, cx: &App) -> bool {
        self.quit_prompt(cx).is_some()
    }

    /// The terminal session for `id`, created on demand but not launched: the
    /// pane launches when it is first shown.
    pub(super) fn ensure_tui(
        &mut self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TuiSession>> {
        if let Some(tui) = self.tui_sessions.get(&id) {
            return Some(tui.clone());
        }
        let summary = self.terminal_summary(id)?.clone();
        // Marks are made on first load, as for a conversation.
        self.session_marks
            .entry(id)
            .or_insert_with(|| cx.new(|_| SessionMark::new(summary.provider.clone())));
        self.tab_marks
            .entry(id)
            .or_insert_with(|| cx.new(|_| SessionMark::tab(summary.provider.clone())));
        if !self.order.contains(&id) {
            self.order.push(id);
        }
        let worktree = self.session_worktrees.get(&id).cloned().flatten();
        let store = self.store.clone();
        let tui = cx.new(|_| TuiSession::new(&summary, store, worktree));
        self.tui_events.insert(
            id,
            cx.subscribe(&tui, move |this, _, event: &TuiEvent, cx| {
                this.tui_event(id, event, cx)
            }),
        );
        self.tui_sessions.insert(id, tui.clone());
        Some(tui)
    }

    /// Start the terminal sessions on screen and end those that left the workspace.
    pub(super) fn sync_tui(&mut self, shown: &HashSet<i64>, cx: &mut Context<Self>) {
        for &id in shown {
            let created = !self.tui_sessions.contains_key(&id);
            if let Some(tui) = self.ensure_tui(id, cx) {
                // A pane that comes up focused takes the keyboard, as a chat's composer does.
                let focus = created && self.workspace.active == Some(id) && !self.review_focused;
                tui.update(cx, |tui, cx| {
                    tui.start(cx);
                    if focus {
                        tui.focus_when_running(cx);
                    }
                });
            }
        }
        // A focus request that outlived its moment (the user moved on while the
        // CLI was starting) must not steal the keyboard when the CLI appears.
        let focusing = !self.app_view_open()
            && self.settings.is_none()
            && self.app_search.is_none()
            && !self.mono_active(cx)
            && !self.review_focused;
        for (&id, tui) in &self.tui_sessions {
            if !(focusing && self.workspace.active == Some(id)) {
                tui.update(cx, |tui, _| tui.cancel_focus());
            }
        }
        let open: HashSet<i64> = self.workspace.open_sessions().into_iter().collect();
        let gone: Vec<i64> = self
            .tui_sessions
            .keys()
            .copied()
            .filter(|id| !open.contains(id))
            .collect();
        for id in gone {
            self.stop_tui(id, cx);
        }
    }

    /// Ends the CLI and forgets the pane. The session row stays: reopening it
    /// resumes the conversation.
    pub(super) fn stop_tui(&mut self, id: i64, cx: &mut Context<Self>) {
        self.tui_events.remove(&id);
        let Some(tui) = self.tui_sessions.remove(&id) else {
            return;
        };
        tui.update(cx, |tui, cx| tui.kill(cx));
        // A run waiting on this pane can never hear from it again.
        self.automation_tui_gone(id, cx);
        self.refresh_session_mark_idle(id, cx);
    }

    /// The pane is gone, so its marks and busy state go quiet.
    fn refresh_session_mark_idle(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.busy_sessions.remove(&id) {
            self.push_inbox_busy(cx);
        }
        self.unseen_finished.remove(&id);
        let Some(provider) = self.session_summaries.get(&id).map(|s| s.provider.clone()) else {
            return;
        };
        if let Some(mark) = self.session_marks.get(&id) {
            mark.update(cx, |mark, cx| mark.set(&provider, MarkState::Idle, cx));
        }
        if let Some(mark) = self.tab_marks.get(&id) {
            mark.update(cx, |mark, cx| mark.set(&provider, MarkState::Idle, cx));
        }
    }

    fn tui_event(&mut self, id: i64, event: &TuiEvent, cx: &mut Context<Self>) {
        self.automation_tui_event(id, event, cx);
        match event {
            TuiEvent::Started => {}
            TuiEvent::Busy(_) | TuiEvent::Exited(_) => {
                self.refresh_session_mark(id, cx);
                self.invalidate_chrome(cx);
            }
            TuiEvent::Conversation(provider_id) => {
                // A saved conversation id is what lists a terminal session.
                if let Some(summary) = self.session_summaries.get_mut(&id) {
                    summary.has_messages = provider_id.is_some();
                    summary.provider_id = provider_id.clone();
                }
                self.invalidate_chrome(cx);
            }
            TuiEvent::Title(title) => self.tui_title(id, title, cx),
            TuiEvent::Close => {
                cx.spawn(async move |this, cx| {
                    let _ =
                        this.update_in(cx, |this, window, cx| this.close_tui_pane(id, window, cx));
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// The CLI names the tab, unless the user or the app already did.
    fn tui_title(&mut self, id: i64, title: &str, cx: &mut Context<Self>) {
        let (Some(summary), Some(tui)) =
            (self.session_summaries.get(&id), self.tui_sessions.get(&id))
        else {
            return;
        };
        let current = summary.title.clone();
        if title == current {
            return;
        }
        let last = tui.read(cx).last_cli_title.clone();
        if !monocode_core::tui_launch::can_retitle(
            &current,
            last.as_deref(),
            Kind::from_id(&summary.provider),
        ) {
            return;
        }
        tui.update(cx, |tui, _| tui.last_cli_title = Some(title.to_owned()));
        if let Some(summary) = self.session_summaries.get_mut(&id) {
            summary.title = title.to_owned();
        }
        let title = title.to_owned();
        // Only while the stored title is still the one this replaced, so a rename
        // that lands first is kept.
        self.store_write("retitle the session", cx, move |store| {
            store.replace_title(id, &current, &title).map(|_| ())
        });
        self.invalidate_chrome(cx);
    }

    /// The CLI ran for a while and exited cleanly: close its pane, or its tab, or
    /// turn the project's last tab into a blank chat. The session is kept. A
    /// terminal tab is always closable (`tab_can_close`), so this is `close_pane`.
    pub(super) fn close_tui_pane(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.workspace.tab_of(id) else {
            return;
        };
        // An exit in the background must not pull the user out of the Inbox,
        // Settings, a search or a Mono: `close_pane` would open the sibling.
        let elsewhere = self.app_view_open()
            || self.settings.is_some()
            || self.app_search.is_some()
            || self.mono_active(cx);
        if elsewhere && self.workspace.layout(tab).is_split() {
            let before = self.active_leaves();
            if self.workspace.close_pane(id) {
                self.history
                    .prune(&self.workspace.open_sessions(), self.workspace.active);
                self.navigation_revision += 1;
                self.begin_pane_motion(before, cx);
                self.fit_sidebar(cx);
                self.persist(cx);
                cx.notify();
            }
            return;
        }
        // The last tab is replaced by a blank session; set only around this call,
        // which creates it synchronously, so it cannot reach an unrelated one.
        self.blank_is_chat = true;
        self.close_pane(id, window, cx);
        self.blank_is_chat = false;
    }

    /// Focus goes to the CLI; before it is running, as soon as it is.
    pub(super) fn focus_tui(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // The pane may not exist yet (it is made by `persist`, after the focus
        // call of an open or restore): `focus` then waits for the CLI to run.
        let Some(tui) = self.ensure_tui(id, cx) else {
            return false;
        };
        tui.update(cx, |tui, cx| tui.focus(window, cx));
        true
    }

    /// A session's body: its chat, or its CLI in place of the chat. A terminal
    /// session whose pane is not made yet draws nothing.
    pub(super) fn session_body(
        &mut self,
        id: i64,
        chat: Option<Entity<Conversation>>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if !self.is_terminal(id) {
            return self.chat_with_background(id, chat, cx);
        }
        let body = div().flex_1().min_h_0().relative().overflow_hidden();
        match self.tui_sessions.get(&id) {
            Some(tui) => body.child(tui.clone().cached(StyleRefinement::default().size_full())),
            None => body,
        }
    }
}
