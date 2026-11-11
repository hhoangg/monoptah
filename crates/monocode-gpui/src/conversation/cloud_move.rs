//! "Continue in cloud" from the conversation's side (design 4.2.2, 4.5): the composer menu
//! entry, the transcript divider, and the reload of the session settings after a move.
//!
//! The flip writes the session's settings in the store (the summary path stores a handoff and
//! resets the model). An open conversation keeps its settings in memory and would miss them,
//! or overwrite them on its next save, so the shell tells it to reload when a move ends.
use super::*;

/// What the shell lets the Workspace menu offer for this session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MoveOffer {
    /// Not a session that can move (new, remote, already in the cloud, a cloud start).
    #[default]
    Hidden,
    /// "Continue in cloud..."
    Offer,
    /// A move of this session is unfinished: "Move in progress...".
    InProgress,
}

/// Shown above the composer and as the refusal while a move of this session is unfinished.
pub(crate) const MOVE_PAUSED: &str =
    "This conversation is being moved to the cloud; sending is paused.";

/// Emitted by the banner's Cancel move button; the shell opens the dialog and cancels.
pub(crate) struct CancelCloudMove;
impl EventEmitter<CancelCloudMove> for Conversation {}

/// Emitted by the Workspace menu entry; the shell opens the move dialog.
pub(crate) struct ContinueInCloud;
impl EventEmitter<ContinueInCloud> for Conversation {}

impl Conversation {
    /// A stop was requested and the turn has not wound down yet.
    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping
    }

    /// The shell says a move of this session is unfinished (from `begin_move` until the flip is
    /// reloaded or the move is cancelled): sends, queued follow-ups and settings writes wait.
    pub(crate) fn set_move_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if self.move_locked != locked {
            self.move_locked = locked;
            cx.notify();
        }
    }
    /// The shell says an agent started the unfinished move (the banner says so).
    pub(crate) fn set_move_by_agent(&mut self, by_agent: bool, cx: &mut Context<Self>) {
        if self.move_by_agent != by_agent {
            self.move_by_agent = by_agent;
            cx.notify();
        }
    }
    #[cfg(test)]
    pub(crate) fn move_by_agent(&self) -> bool {
        self.move_by_agent
    }
    pub(crate) fn move_locked(&self) -> bool {
        self.move_locked
    }
    /// The flip happened: the stored settings changed under this copy.
    pub(crate) fn mark_move_flipped(&mut self) {
        self.move_settings_stale = true;
    }

    /// The banner above the composer while a move is unfinished.
    pub(super) fn move_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.move_locked {
            return None;
        }
        let button = |id: &'static str, label: &'static str| {
            crate::settings::widgets::secondary_button(id, label, false, false)
                .debug_selector(move || id.into())
                .child(label)
        };
        let card = composer_shelf::card("cloud-move-notice", self.classic_layout)
            .debug_selector(|| "cloud-move-notice".into())
            .role(Role::Group)
            .aria_label("Move to the cloud in progress")
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon("cloud-upload", 14.).text_color(warning_color()))
                    .child(
                        div()
                            .debug_selector(|| "cloud-move-notice-text".into())
                            .text_color(warning_color())
                            .child(MOVE_PAUSED),
                    ),
            )
            .children(self.move_by_agent.then(|| {
                div()
                    .debug_selector(|| "cloud-move-by-agent".into())
                    .text_color(warning_color())
                    .child(crate::cloud_move_dialog::STARTED_BY_AGENT)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("cloud-move-show", "Show move")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ContinueInCloud))),
                    )
                    .child(
                        button("cloud-move-cancel", "Cancel move")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CancelCloudMove))),
                    ),
            );
        Some(
            composer_shelf::shelf("cloud-move-notice-shelf")
                .px(px(8.))
                .child(card)
                .into_any_element(),
        )
    }

    /// The shell tells whether this session can move to the cloud.
    pub(crate) fn set_move_offer(&mut self, offer: MoveOffer, cx: &mut Context<Self>) {
        if self.move_offer != offer {
            self.move_offer = offer;
            cx.notify();
        }
    }

    /// The Workspace menu section for a session that already has messages.
    pub(super) fn move_menu_items(&self) -> Vec<MenuItem> {
        let label = match self.move_offer {
            MoveOffer::Hidden => return Vec::new(),
            MoveOffer::Offer => "Continue in cloud\u{2026}",
            MoveOffer::InProgress => "Move in progress\u{2026}",
        };
        vec![
            MenuItem::Separator,
            MenuItem::Header("Cloud".into()),
            MenuAction::new("workspace:cloud-move", label)
                .icon("cloud-upload")
                .into(),
        ]
    }

    /// The text of the transcript divider once the session moved; `None` removes it.
    pub(crate) fn set_cloud_moved(&mut self, label: Option<String>, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |transcript, cx| transcript.set_cloud_moved(label, cx));
    }

    /// Reads the session settings from the store again, after a move changed them there.
    pub(crate) fn reload_settings(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let id = self.summary.id;
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store).and_then(|s| s.agent_settings(id)) });
        cx.spawn(async move |this, cx| {
            let saved = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(Some(saved)) = saved {
                    this.apply_saved_settings(saved, cx);
                }
            });
        })
        .detach();
    }

    /// Replaces the settings in memory with `saved`, keeping what is never stored.
    pub(crate) fn apply_saved_settings(
        &mut self,
        mut saved: monocode_core::agent::Settings,
        cx: &mut Context<Self>,
    ) {
        self.move_settings_stale = false;
        self.model = saved.model.clone();
        saved.mono = self.settings.mono.take();
        saved.environment = std::mem::take(&mut self.settings.environment);
        self.settings = saved;
        self.attach_account(cx);
        cx.notify();
    }
}

#[cfg(test)]
impl Conversation {
    pub(crate) fn settings_for_test(&self) -> &monocode_core::agent::Settings {
        &self.settings
    }
    pub(crate) fn save_settings_for_test(&mut self, cx: &mut Context<Self>) {
        self.save_settings(cx)
    }
    /// The labels of the move entries of the Workspace menu.
    pub(crate) fn move_menu_items_for_test(&self) -> Vec<String> {
        self.move_menu_items()
            .into_iter()
            .filter_map(|item| match item {
                MenuItem::Action(action) => Some(action.label.to_string()),
                _ => None,
            })
            .collect()
    }
}
