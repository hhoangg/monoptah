//! A local session whose location is Cloud (design 4.2.1): what its conversation view shows
//! and refuses. The shell pushes the location in (`set_cloud_location`); the view never reads
//! the store on the paint path.
//!
//! * Rewind-like actions are gated by the move point (`checkpoint::gate`).
//! * The durable row-append error is a notice with a "Reset to local" action. It asks for an
//!   explicit confirmation first, because the reset does not reconcile the transcript.
use super::*;
use monocode_core::checkpoint::gate::{self, Gate, Operation};
use monocode_core::cloud_sessions::LocatedSession;

/// The confirmation text of "Reset to local". It must say what the reset does not do.
pub(crate) const RESET_CONFIRM: &str = "This stops syncing with the cloud workspace and runs the conversation on this computer again. The messages are not reconciled: anything written in the cloud after the last sync is not copied here, and the local provider session does not know about those cloud turns. The copy in the cloud workspace stays there.";

/// The status line after a confirmed reset.
pub(crate) const RESET_DONE: &str =
    "Back on this computer. Messages written in the cloud after the last sync are not included.";

/// Emitted after a confirmed reset, so the shell refreshes the cloud marks.
pub(crate) struct CloudLocationReset;
impl EventEmitter<CloudLocationReset> for Conversation {}

#[derive(Default)]
pub(super) struct State {
    pub(super) located: Option<LocatedSession>,
    /// "Reset to local" was clicked and the confirmation is showing. Nothing is reset until
    /// it is confirmed.
    pub(super) confirming: bool,
    resetting: bool,
}

impl Conversation {
    /// The shell tells whether this session lives in the cloud and, if so, its move point and
    /// durable sync error.
    pub(crate) fn set_cloud_location(
        &mut self,
        located: Option<LocatedSession>,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_location.located == located {
            return;
        }
        let moved = located.as_ref().map(|l| l.moved_at_row);
        if located.is_none() {
            self.cloud_location.confirming = false;
        }
        // The recorded changes of a session in the cloud are from the turns that ran here
        // before the move, so Undo is gated like a rewind to the first of them.
        let undo_blocked = located.as_ref().and_then(|l| {
            gate::gate_at(Some(l.moved_at_row), Operation::RestoreFiles, 0)
                .reason()
                .filter(|_| l.moved_at_row > 0)
        });
        self.review
            .update(cx, |review, cx| review.set_undo_blocked(undo_blocked, cx));
        self.cloud_location.located = located;
        self.transcript
            .update(cx, |transcript, cx| transcript.set_cloud_move(moved, cx));
        cx.notify();
    }
    /// The move point the shell last pushed in, for tests.
    #[cfg(test)]
    pub(crate) fn cloud_move_for_test(&self) -> Option<usize> {
        self.cloud_location.located.as_ref().map(|l| l.moved_at_row)
    }
    /// Whether `operation` may start at store row `row` of this session.
    pub(super) fn rewind_gate(&self, operation: Operation, row: usize) -> Gate {
        gate::gate_at(
            self.cloud_location.located.as_ref().map(|l| l.moved_at_row),
            operation,
            row,
        )
    }
    /// The Reset to local button: opens the confirmation. Never resets by itself.
    pub(crate) fn request_reset_to_local(&mut self, cx: &mut Context<Self>) {
        if self.cloud_location.located.is_some() && !self.busy && !self.cloud_location.resetting {
            self.cloud_location.confirming = true;
            cx.notify();
        }
    }
    pub(crate) fn cancel_reset_to_local(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.cloud_location.confirming) {
            cx.notify();
        }
    }
    /// Runs the reset, only after `request_reset_to_local` asked for confirmation. Returns
    /// whether it started.
    pub(crate) fn confirm_reset_to_local(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.cloud_location.confirming
            || self.cloud_location.located.is_none()
            || self.cloud_location.resetting
        {
            return false;
        }
        self.cloud_location.confirming = false;
        self.cloud_location.resetting = true;
        let store = self.store.clone();
        let id = self.summary.id;
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.reset_cloud_location_to_local(id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.cloud_location.resetting = false;
                match result {
                    Ok(()) => {
                        this.set_cloud_location(None, cx);
                        this.status = RESET_DONE.into();
                        cx.emit(CloudLocationReset);
                        cx.emit(SessionChanged);
                    }
                    Err(error) => this.status = format!("Could not reset to local: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        true
    }
    /// Whether the sync notice (or its confirmation) is showing.
    pub(crate) fn cloud_sync_notice_visible(&self) -> bool {
        self.cloud_location
            .located
            .as_ref()
            .is_some_and(|l| l.sync_error.is_some() || self.cloud_location.confirming)
    }
    /// The notice above the composer for a Cloud session whose last row append failed.
    pub(super) fn cloud_sync_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let located = self.cloud_location.located.as_ref()?;
        if !self.cloud_sync_notice_visible() {
            return None;
        }
        let confirming = self.cloud_location.confirming;
        let title = if confirming {
            "Reset to local?"
        } else {
            "Cloud sync problem"
        };
        let message = if confirming {
            RESET_CONFIRM.to_owned()
        } else {
            located.sync_error.clone().unwrap_or_default()
        };
        let button = |id: &'static str, label: &'static str| {
            crate::settings::widgets::secondary_button(
                id,
                label,
                confirming && id == "cloud-reset-confirm",
                false,
            )
            .debug_selector(move || id.into())
            .child(label)
        };
        let actions = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .when(!confirming, |d| {
                d.child(
                    button("cloud-reset-to-local", "Reset to local")
                        .on_click(cx.listener(|this, _, _, cx| this.request_reset_to_local(cx))),
                )
            })
            .when(confirming, |d| {
                d.child(
                    button("cloud-reset-cancel", "Cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_reset_to_local(cx))),
                )
                .child(
                    button("cloud-reset-confirm", "Reset to local").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.confirm_reset_to_local(cx);
                        },
                    )),
                )
            });
        let card = composer_shelf::card("cloud-sync-notice", self.classic_layout)
            .debug_selector(|| "cloud-sync-notice".into())
            .role(Role::Group)
            .aria_label(title)
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon("circle-alert", 14.).text_color(warning_color()))
                    .child(
                        div()
                            .debug_selector(|| "cloud-sync-notice-title".into())
                            .text_color(warning_color())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "cloud-sync-notice-message".into())
                    .text_color(content(0.55))
                    .child(message),
            )
            .child(actions);
        Some(
            composer_shelf::shelf("cloud-sync-notice-shelf")
                .px(px(8.))
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use monocode_core::store::Store;
    use std::sync::Mutex;

    pub(super) fn located(sync_error: Option<&str>) -> LocatedSession {
        LocatedSession {
            id: 1,
            moved_at_row: 10,
            sync_error: sync_error.map(str::to_owned),
        }
    }
    #[test]
    fn the_confirmation_says_the_transcript_is_not_reconciled() {
        assert!(RESET_CONFIRM.contains("not reconciled"));
        assert!(!RESET_CONFIRM.contains('\u{2014}') && !RESET_CONFIRM.contains('\u{2013}'));
        assert!(!RESET_DONE.contains('\u{2014}') && !RESET_DONE.contains('\u{2013}'));
    }
    #[gpui::test]
    fn a_reset_needs_the_confirmation_and_then_clears_location_cursor_and_links(
        cx: &mut TestAppContext,
    ) {
        let mut store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        let rows: Vec<monocode_core::transcript::Row> = (0..10)
            .map(|i| monocode_core::transcript::Row {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                text: format!("local {i}"),
                first: true,
                sent_at: None,
            })
            .collect();
        store.write_rows(session.id, 0, &rows).unwrap();
        store
            .set_cloud_location(
                session.id,
                &monocode_core::store::SessionLocation::Cloud(
                    monocode_core::store::CloudLocation {
                        cloud: monocode_core::cloud_sessions::CloudRef::codespace("octo-abc"),
                        environment_id: "env".into(),
                        project_id: "project".into(),
                        host_session_id: "host-1".into(),
                        remote_cwd: "/host/repo".into(),
                        local_cwd: "/Users/me/repo".into(),
                        moved_at_row: 10,
                        pushed_branch: None,
                    },
                ),
            )
            .unwrap();
        store.save_setting("cloud-synced:env:host-1", "3").unwrap();
        store
            .record_cloud_sync_error(session.id, "Host unreachable")
            .unwrap();
        let id = session.id;
        let database = Arc::new(Mutex::new(store));
        let chat = cx.new(|cx| Conversation::new(session, database.clone(), cx));
        chat.update(cx, |chat, cx| {
            chat.set_cloud_location(Some(located(Some("Host unreachable"))), cx);
            assert!(chat.cloud_sync_notice_visible());
            // Confirming without asking first does nothing.
            assert!(!chat.confirm_reset_to_local(cx));
            // Asking only opens the confirmation; the location is untouched.
            chat.request_reset_to_local(cx);
            assert!(chat.cloud_location.confirming);
            assert!(
                locked(&database)
                    .unwrap()
                    .cloud_location(id)
                    .unwrap()
                    .is_cloud()
            );
            // Cancel closes it and still resets nothing.
            chat.cancel_reset_to_local(cx);
            assert!(!chat.confirm_reset_to_local(cx));
            assert!(
                locked(&database)
                    .unwrap()
                    .cloud_location(id)
                    .unwrap()
                    .is_cloud()
            );
            chat.request_reset_to_local(cx);
            assert!(chat.confirm_reset_to_local(cx));
        });
        cx.run_until_parked();
        chat.update(cx, |chat, _| {
            assert!(chat.cloud_location.located.is_none());
            assert_eq!(chat.status, RESET_DONE);
        });
        let store = locked(&database).unwrap();
        assert_eq!(
            store.cloud_location(id).unwrap(),
            monocode_core::store::SessionLocation::Local
        );
        assert_eq!(store.cloud_synced_revision("env", "host-1").unwrap(), None);
        assert_eq!(store.cloud_sync_error(id).unwrap(), None);
        assert!(store.linked_remote_session(id).unwrap().is_none());
        assert_eq!(store.load(id).unwrap().rows.len(), 10, "no row is touched");
    }
    #[gpui::test]
    fn rewinds_follow_the_move_point_pushed_by_the_shell(cx: &mut TestAppContext) {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/Users/me/repo").unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            assert!(chat.rewind_gate(Operation::EditLastTurn, 0).is_allowed());
            chat.set_cloud_location(Some(located(None)), cx);
            assert!(!chat.cloud_sync_notice_visible(), "no error, no notice");
            assert_eq!(
                chat.rewind_gate(Operation::EditLastTurn, 9).reason(),
                Some(gate::BEFORE_MOVE)
            );
            assert!(chat.rewind_gate(Operation::RestoreFiles, 10).is_allowed());
            assert_eq!(
                chat.rewind_gate(Operation::EditLastTurn, 10).reason(),
                Some(gate::CLOUD_OWNED)
            );
            chat.set_cloud_location(None, cx);
            assert!(chat.rewind_gate(Operation::EditLastTurn, 0).is_allowed());
        });
    }
}
