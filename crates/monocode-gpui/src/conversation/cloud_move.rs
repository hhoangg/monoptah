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

/// Emitted by the Workspace menu entry; the shell opens the move dialog.
pub(crate) struct ContinueInCloud;
impl EventEmitter<ContinueInCloud> for Conversation {}

impl Conversation {
    /// A stop was requested and the turn has not wound down yet.
    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping
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
