//! The `/` menu for a paired web device: what the menu offers this conversation now, and the
//! commands it runs, through the same functions the native composer's menu and send path use.
use super::*;
use monocode_core::slash_commands::{self, Context as MenuContext};

/// The command ids a paired device may run with `commands.run`.
pub(crate) const RUNNABLE: &[&str] = &[
    "plan",
    "operator",
    "orchestrator",
    "draft",
    "btw",
    "compact",
];

impl Conversation {
    /// Why Compact context cannot run now, in a plain sentence.
    pub(super) fn compact_blocked(&self, cx: &gpui::App) -> Option<String> {
        if self.busy || self.stopping || !self.follow_ups.state.messages.is_empty() {
            return Some("Wait for the current turn and queued messages to finish".into());
        }
        if self.editing_last_turn.pending()
            || self.attaching
            || self.input.read(cx).is_composing()
            || self.account_switching
            || self.controls.is_some()
            || self.needs_approval()
            || !self.questions.is_empty()
        {
            return Some("Answer the open question or approval on the desktop first".into());
        }
        let kind = Kind::from_id(&self.summary.provider);
        if !monocode_core::provider::supports_compact(kind) {
            return Some(format!("{} does not support compacting", kind.label()));
        }
        if self.summary.provider_id.is_none() {
            return Some("Send a message before compacting this session".into());
        }
        if self.settings.worktree_removed || self.preparing_worktree || !self.loaded {
            return Some("This session is still opening on the desktop".into());
        }
        None
    }

    /// What the menu knows about this conversation: the modes that are on and why a command
    /// cannot run now. The rules are the ones the desktop's own menus apply.
    pub(crate) fn mobile_menu_context(&self, read_only: bool, cx: &gpui::App) -> MenuContext {
        let mut blocked = std::collections::BTreeMap::new();
        if self.busy || self.stopping {
            blocked.insert(
                "draft".to_owned(),
                "Wait for the current turn to finish".to_owned(),
            );
        } else if self.follow_ups.state.draft.is_some() {
            blocked.insert("draft".to_owned(), "A draft is already saved".to_owned());
        }
        if self.is_remote() {
            for id in ["operator", "orchestrator"] {
                blocked.insert(id.to_owned(), "Not available on remote projects".to_owned());
            }
        }
        if !self.btw_available(cx) {
            blocked.insert(
                "btw".to_owned(),
                "Finish a conversation turn before asking a side question".to_owned(),
            );
        }
        if let Some(reason) = self.compact_blocked(cx) {
            blocked.insert("compact".to_owned(), reason);
        }
        let active = ["plan", "operator", "orchestrator"]
            .into_iter()
            .filter(|id| match *id {
                "plan" => self.settings.plan,
                "operator" => self.settings.operator,
                _ => self.settings.orchestrator,
            })
            .map(str::to_owned)
            .collect();
        MenuContext {
            provider: Some(Kind::from_id(&self.summary.provider)),
            read_only,
            blocked,
            active,
        }
    }

    /// Turns Plan, Operator or Orchestrator on or off for the next message, as the composer's
    /// mode menu does, without touching the text in the desktop's composer. Returns whether the
    /// mode is on afterwards.
    pub(crate) fn mobile_set_mode(
        &mut self,
        id: &str,
        enabled: Option<bool>,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let on = match id {
            "plan" => self.settings.plan,
            "operator" => self.settings.operator,
            "orchestrator" => self.settings.orchestrator,
            _ => return Err("Unknown command".into()),
        };
        if enabled.unwrap_or(!on) != on {
            self.status = String::new();
            if !self.flip_mode(id, cx) {
                return Err(if self.status.is_empty() {
                    "This mode is not available".into()
                } else {
                    self.status.clone()
                });
            }
            self.save_settings(cx);
            cx.emit(SessionChanged);
            cx.notify();
        }
        Ok(match id {
            "plan" => self.settings.plan,
            "operator" => self.settings.operator,
            _ => self.settings.orchestrator,
        })
    }

    /// Saves `text` as this session's draft, as Draft then Enter does in the desktop composer.
    /// A desktop composer that holds text of its own, or a draft that is already saved, is left
    /// alone and the call fails.
    pub(crate) fn mobile_save_draft(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("Write a message to save as a draft".into());
        }
        if self.busy || self.stopping {
            return Err("Wait for the current turn to finish".into());
        }
        if self.follow_ups.state.draft.is_some() {
            return Err("A draft is already saved".into());
        }
        if !self.history_ready() || !self.loaded || self.input.read(cx).is_composing() {
            return Err("This session is still opening on the desktop".into());
        }
        if !self.input.read(cx).content.is_empty() || !self.attachments.is_empty() {
            return Err("The desktop composer holds a message. Send or clear it first".into());
        }
        self.input.update(cx, |input, cx| input.set_text(text, cx));
        self.save_message_draft(cx);
        if self.follow_ups.state.draft.is_none() {
            self.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            return Err("The draft could not be saved".into());
        }
        Ok(())
    }

    /// Opens a side question on the desktop, as `/btw` does.
    pub(crate) fn mobile_side_question(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("Write the side question first".into());
        }
        if !self.btw_available(cx) {
            return Err("Finish a conversation turn before asking a side question".into());
        }
        if !self.input.read(cx).content.is_empty() {
            return Err("The desktop composer holds a message. Send or clear it first".into());
        }
        if self.open_btw(text.trim().to_owned(), false, cx) {
            return Ok(());
        }
        Err(if self.status.is_empty() {
            "A side question is not available now".into()
        } else {
            self.status.clone()
        })
    }

    /// Starts compacting the context, as Compact context does.
    pub(crate) fn mobile_compact(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if !self.history_ready() {
            self.load(cx);
            return Err("This session is still opening on the desktop".into());
        }
        if let Some(reason) = self.compact_blocked(cx) {
            return Err(reason);
        }
        self.compact_context(cx);
        if self.busy {
            Ok(())
        } else {
            Err(if self.status.is_empty() {
                "Compacting could not start".into()
            } else {
                self.status.clone()
            })
        }
    }

    /// Whether `command` is one the web device may run here.
    pub(crate) fn mobile_runnable(command: &str) -> bool {
        RUNNABLE.contains(&command) && slash_commands::BUILTINS.iter().any(|b| b.id == command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use monocode_core::store::Store;
    use std::sync::Mutex;

    fn window(cx: &mut TestAppContext) -> gpui::WindowHandle<Conversation> {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let summary = store.create("claude", "/missing/slash-commands").unwrap();
        cx.update(|cx| {
            input::bind(cx);
            crate::popover::bind(cx);
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                    view.loaded = true;
                    view
                })
            })
            .unwrap()
        })
    }

    #[gpui::test]
    fn modes_switch_exclusively_and_leave_the_desktop_composer_text_alone(cx: &mut TestAppContext) {
        let window = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |view, _, cx| {
                view.input
                    .update(cx, |input, cx| input.set_text("my own draft".into(), cx));
                assert_eq!(view.mobile_set_mode("plan", None, cx), Ok(true));
                assert!(view.settings.plan);
                // Turning on another mode turns the first off.
                assert_eq!(
                    view.mobile_set_mode("orchestrator", Some(true), cx),
                    Ok(true)
                );
                assert!(view.settings.orchestrator && !view.settings.plan);
                // An explicit state that already holds changes nothing.
                assert_eq!(
                    view.mobile_set_mode("orchestrator", Some(true), cx),
                    Ok(true)
                );
                assert_eq!(view.mobile_set_mode("operator", Some(false), cx), Ok(false));
                assert_eq!(view.mobile_set_mode("orchestrator", None, cx), Ok(false));
                assert!(!view.settings.orchestrator);
                assert_eq!(view.input.read(cx).content, "my own draft");
                assert!(view.mobile_set_mode("compact", None, cx).is_err());
                // The menu reports what is on.
                view.mobile_set_mode("operator", Some(true), cx).unwrap();
                let context = view.mobile_menu_context(false, cx);
                assert_eq!(context.active, ["operator"]);
                assert_eq!(context.provider, Some(Kind::Claude));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_draft_is_saved_from_the_text_and_never_over_the_desktop_composer(cx: &mut TestAppContext) {
        let window = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |view, _, cx| {
                assert!(view.mobile_save_draft("   ".into(), cx).is_err());
                view.input.update(cx, |input, cx| {
                    input.set_text("typed on the desktop".into(), cx)
                });
                let error = view
                    .mobile_save_draft("from the web".into(), cx)
                    .unwrap_err();
                assert!(error.contains("desktop composer"));
                assert!(view.follow_ups.state.draft.is_none());
                assert_eq!(view.input.read(cx).content, "typed on the desktop");
                view.input
                    .update(cx, |input, cx| input.set_text(String::new(), cx));
                view.busy = true;
                assert!(view.mobile_save_draft("from the web".into(), cx).is_err());
                assert!(
                    view.mobile_menu_context(false, cx)
                        .blocked
                        .contains_key("draft")
                );
                view.busy = false;
                view.mobile_save_draft("from the web".into(), cx).unwrap();
                assert_eq!(
                    view.follow_ups
                        .state
                        .draft
                        .as_ref()
                        .map(|m| m.text.as_str()),
                    Some("from the web")
                );
                assert!(view.input.read(cx).content.is_empty());
                // A second draft is refused with a reason the menu shows too.
                assert!(view.mobile_save_draft("again".into(), cx).is_err());
                assert_eq!(
                    view.mobile_menu_context(false, cx)
                        .blocked
                        .get("draft")
                        .map(String::as_str),
                    Some("A draft is already saved")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn compact_and_side_questions_say_why_they_are_not_available(cx: &mut TestAppContext) {
        let window = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |view, _, cx| {
                // A session that never ran has nothing to compact or to ask about.
                let context = view.mobile_menu_context(false, cx);
                assert_eq!(
                    context.blocked.get("compact").map(String::as_str),
                    Some("Send a message before compacting this session")
                );
                assert!(context.blocked.contains_key("btw"));
                assert!(view.mobile_compact(cx).is_err());
                assert!(!view.busy);
                assert!(view.mobile_side_question("why?".into(), cx).is_err());
                // The rows carry those reasons.
                let rows = slash_commands::list(&context, &[]);
                let compact = rows.iter().find(|row| row.id == "compact").unwrap();
                assert!(!compact.available && compact.reason.is_some());
                assert!(rows.iter().find(|row| row.id == "plan").unwrap().available);
                view.busy = true;
                assert_eq!(
                    view.compact_blocked(cx).as_deref(),
                    Some("Wait for the current turn and queued messages to finish")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_draft_prefixed_message_from_a_device_saves_a_draft_and_runs_no_agent(
        cx: &mut TestAppContext,
    ) {
        let window = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |view, _, cx| {
                // What `session.start` and `conversation.send` hand to `mobile_submit`.
                view.mobile_submit("/draft call me later".into(), Vec::new(), cx);
                assert!(!view.busy);
                assert_eq!(
                    view.follow_ups
                        .state
                        .draft
                        .as_ref()
                        .map(|m| m.text.as_str()),
                    Some("call me later")
                );
                assert!(view.input.read(cx).content.is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_side_question_needs_text_and_a_finished_turn(cx: &mut TestAppContext) {
        let window = window(cx);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |view, _, cx| {
                assert_eq!(
                    view.mobile_side_question("  ".into(), cx),
                    Err("Write the side question first".to_owned())
                );
                assert!(
                    view.mobile_side_question("why?".into(), cx)
                        .unwrap_err()
                        .contains("Finish a conversation turn")
                );
            })
            .unwrap();
    }

    #[test]
    fn only_the_commands_with_a_desktop_function_are_runnable() {
        for id in [
            "plan",
            "operator",
            "orchestrator",
            "draft",
            "btw",
            "compact",
        ] {
            assert!(Conversation::mobile_runnable(id), "{id}");
        }
        for id in [
            "add-to-folder",
            "mcp",
            "stage",
            "commit",
            "visualize",
            "x",
            "",
        ] {
            assert!(!Conversation::mobile_runnable(id), "{id}");
        }
    }
}
