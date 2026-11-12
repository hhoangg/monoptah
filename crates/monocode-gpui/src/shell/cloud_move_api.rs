//! The app API of the session moves (rule D11): `sessions.moveToCloud`, `sessions.moveToLocal`,
//! `sessions.moveStatus` and `sessions.cancelMove`, called by a Mono through the Mono host.
//!
//! Everything here goes through the same paths as the UI, so the API has the same safety rules
//! and can only narrow the user's choices:
//!
//! * a move to the cloud picks one of the targets the move dialog offers
//!   ([`Shell::move_targets`]), starts a normal `MoveRecord` (`started_by: api`, visible in the
//!   sidebar and in the move dialog with Cancel) and runs it with the same worker as Confirm;
//! * `kit: "none"` turns the kit off for the workspace the move creates; nothing can turn it on
//!   or add items;
//! * a return only OPENS the Continue locally dialog (the user confirms it); it runs `CloudSessions::continue_locally`, so
//!   its refusals (a turn running here or on the Host, a workspace in use) are the same;
//! * a cancel never deletes a workspace;
//! * a call returns at once: the work runs on a worker and the state is read with
//!   `sessions.moveStatus`. Nothing here is reached by a background path, so no workspace is
//!   woken except by the explicit call.
use super::cloud_move::NewMove;
use super::*;
use crate::conversation::MoveOffer;
use monocode_core::cloud_sessions::move_session::{BUSY, MOVE_RUNNING, NO_MOVE, StartedBy};
use monocode_core::files::path_key;
use monocode_core::mono::move_api::{self, MoveToCloud};
use serde_json::{Value, json};

/// The session is not one of the project of the caller.
const OUTSIDE: &str = "Session is outside the assigned project";

impl Shell {
    fn api_in_project(&self, id: i64, project: &str) -> bool {
        let key = path_key(project);
        match self.cloud_marks.get(&id) {
            Some(mark) => path_key(&mark.local_project) == key,
            None => self
                .session_summaries
                .get(&id)
                .is_some_and(|s| path_key(&s.cwd) == key),
        }
    }

    /// `sessions.moveToCloud`. Validation and the refusals come before anything is created, and
    /// the move itself runs on a worker.
    pub(crate) fn api_move_to_cloud(
        &mut self,
        project: &str,
        request: MoveToCloud,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let id = request.session;
        if !self.session_summaries.contains_key(&id) || !self.api_in_project(id, project) {
            return Err(OUTSIDE.into());
        }
        match self.move_offer(id) {
            MoveOffer::Offer => {}
            MoveOffer::InProgress => return Err(MOVE_RUNNING.into()),
            MoveOffer::Hidden => {
                return Err("This conversation cannot be moved to the cloud: it has no messages yet, it already lives in the cloud or it runs on another machine.".into());
            }
        }
        if self.session_is_busy(id, cx) {
            return Err(BUSY.into());
        }
        let offered: Vec<_> = self
            .move_targets(id)
            .into_iter()
            .map(|choice| choice.target)
            .collect();
        let target = move_api::resolve_target(&request, &offered)?;
        self.api_move_errors.remove(&id);
        // Visible even when the session is not open: a notice in the window and the banner.
        let title = self
            .session_summaries
            .get(&id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        self.error = Some(format!("An agent started moving {title} to the cloud."));
        let mut run = cloud_move::MoveRun::new(None);
        run.by_api = true;
        self.cloud_moves.insert(id, run);
        self.start_move_worker(
            id,
            Some(NewMove {
                target,
                started_by: StartedBy::Api,
                kit_off: request.kit_off,
            }),
            cx,
        );
        self.sync_cloud_moves(cx);
        cx.notify();
        Ok(json!({
            "session": id,
            "direction": "toCloud",
            "state": "running",
            "startedBy": "api",
        }))
    }

    /// `sessions.moveStatus`: reads only.
    pub(crate) fn api_move_status(&self, project: &str, id: i64) -> Result<Value, String> {
        if let Some(new) = self.returned_as(id, project) {
            return Ok(move_api::returned_json(id, new));
        }
        if !self.session_summaries.contains_key(&id) || !self.api_in_project(id, project) {
            return Err(OUTSIDE.into());
        }
        if let Some((state, error)) = self.cloud_continue.api_status(id) {
            return Ok(move_api::return_json(id, state, error.as_deref()));
        }
        if let Some(run) = self.cloud_moves.get(&id) {
            return Ok(match &run.record {
                Some(record) => move_api::status_json(id, record, run.running),
                None => json!({
                    "session": id,
                    "direction": "toCloud",
                    "state": if run.running { "running" } else { "paused" },
                    "startedBy": if run.by_api { "api" } else { "user" },
                }),
            });
        }
        let in_cloud = self.cloud_located.contains_key(&id) || self.cloud_marks.contains_key(&id);
        Ok(move_api::idle_json(
            id,
            in_cloud,
            self.api_move_errors.get(&id).map(String::as_str),
        ))
    }

    /// `sessions.cancelMove`: the same Cancel as the dialog's, without deleting a workspace. A
    /// workspace the move created stays; the user decides about it in the app.
    pub(crate) fn api_cancel_move(
        &mut self,
        project: &str,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        if !self.session_summaries.contains_key(&id) || !self.api_in_project(id, project) {
            return Err(OUTSIDE.into());
        }
        let Some(run) = self.cloud_moves.get(&id) else {
            return Err(NO_MOVE.into());
        };
        // Only a move an agent started: the user's own move is the user's to cancel.
        let by_agent = run.by_api
            || run
                .record
                .as_ref()
                .is_some_and(|r| r.started_by == StartedBy::Api);
        if !by_agent {
            return Err("That move was started by the user; only the user can cancel it.".into());
        }
        self.cancel_cloud_move(id, false, cx);
        Ok(json!({"session": id, "state": "cancelling"}))
    }

    /// `sessions.moveToLocal`: opens the Continue locally dialog (so the user sees it) and runs
    /// it. The result is read with `sessions.moveStatus`.
    pub(crate) fn api_move_to_local(
        &mut self,
        project: &str,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        // The project comes first: an error must not tell whether a session of another project
        // is local or in the cloud.
        if !self.session_summaries.contains_key(&id) || !self.api_in_project(id, project) {
            return Err(OUTSIDE.into());
        }
        if !self.cloud_marks.contains_key(&id) {
            return Err("This session does not live in the cloud.".into());
        }
        if self.cloud_moves.contains_key(&id) {
            return Err(MOVE_RUNNING.into());
        }
        if self.workspace_busy_of(id) || self.cloud_busy.contains(&id) {
            return Err(cloud::WORKSPACE_BUSY.into());
        }
        if self.cloud_continue.dialog.is_some() {
            return Err(
                "Another return to this computer is open. Try again when it finishes.".into(),
            );
        }
        if self.session_metadata(id, cx).busy {
            return Err("A turn is running. Wait for it to finish, then continue locally.".into());
        }
        // Only OPENS the confirmation: the return changes the user's working tree, so the user
        // presses Continue. Nothing runs before that.
        self.open_continue_locally(id, window, cx);
        if self.cloud_continue.api_status(id).is_none() {
            return Err("The confirmation could not be opened.".into());
        }
        Ok(move_api::return_json(id, "awaitingConfirmation", None))
    }

    /// Remembers that cloud session `old` came back as local session `new` (bounded), so
    /// `sessions.moveStatus` can still answer for the old id.
    pub(super) fn remember_returned(&mut self, old: i64, new: i64) {
        const KEEP: usize = 32;
        let Some(project) = self
            .cloud_marks
            .get(&old)
            .map(|mark| mark.local_project.clone())
        else {
            return;
        };
        self.api_returned.retain(|(o, _, _)| *o != old);
        self.api_returned.push_back((old, new, project));
        while self.api_returned.len() > KEEP {
            self.api_returned.pop_front();
        }
    }

    fn returned_as(&self, old: i64, project: &str) -> Option<i64> {
        let key = path_key(project);
        self.api_returned
            .iter()
            .find(|(o, _, p)| *o == old && path_key(p) == key)
            .map(|(_, new, _)| *new)
    }
}
