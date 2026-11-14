//! `sessions.archive` with `cloud`, for the Operator app API and for Monos: the same flow as
//! archiving a cloud session in the window (save a local copy, then keep, stop or delete the
//! workspace), driven by an app call instead of the dialog.
use super::cloud::{
    ArchiveChoice, CANCELLED, ConvertFn, GroupOutcome, NEEDS_FILES_ONLY, NEEDS_REVIEW, Then,
    WORKSPACE_BUSY, archive_groups, group_by_codespace,
};
use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// Why an app archive did not finish, and whether the session got archived anyway (its local
/// copy was saved), so a caller undoes its own bookkeeping only when nothing was archived.
#[derive(Debug)]
pub(crate) struct ArchiveFailure {
    pub message: String,
    pub archived: bool,
}
impl ArchiveFailure {
    fn untouched(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            archived: false,
        }
    }
}

impl ArchiveChoice {
    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "keep" => Ok(Self::Keep),
            "stop" => Ok(Self::Stop),
            "delete" => Ok(Self::Delete),
            _ => Err("cloud must be \"keep\", \"stop\" or \"delete\"".into()),
        }
    }
    fn word(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Stop => "stop",
            Self::Delete => "delete",
        }
    }
}

impl Shell {
    /// The instance action of `sessions.delete` with `cloud: "stop" | "delete"` for a cloud
    /// session; `None` otherwise (the instance is left alone).
    pub(super) fn app_delete_instance_task(
        &self,
        id: i64,
        input: &Value,
    ) -> Option<super::project_removal::InstanceTask> {
        let action = match input["cloud"].as_str()? {
            "stop" => monocode_core::cloud_sessions::InstanceAction::Stop,
            "delete" => monocode_core::cloud_sessions::InstanceAction::Delete,
            _ => return None,
        };
        // The window's mark, else the stored binding (a session the sidebar has not marked yet).
        let target = match self.cloud_marks.get(&id) {
            Some(mark) => mark.cloud(),
            None => locked(&self.store)
                .ok()?
                .remote_session(id)
                .ok()??
                .project
                .cloud()?,
        };
        Some(super::project_removal::InstanceTask {
            target,
            action,
            selected: vec![id],
        })
    }

    /// Archives cloud session `id` and applies `choice` ("keep", "stop" or "delete") to its
    /// workspace. Nothing is stopped or deleted unless the local copy is complete. `reply` gets
    /// the result, or why the workspace was left as it was.
    pub(crate) fn app_archive_cloud(
        &mut self,
        id: i64,
        choice: &str,
        backup: bool,
        reply: async_channel::Sender<Result<Value, ArchiveFailure>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let choice = match ArchiveChoice::parse(choice) {
            Ok(choice) => choice,
            Err(error) => {
                let _ = reply.try_send(Err(ArchiveFailure::untouched(error)));
                return;
            }
        };
        let Some(mark) = self.cloud_marks.get(&id).cloned() else {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(
                "cloud only applies to a GitHub Codespaces or Docker Sandbox session",
            )));
            return;
        };
        let target = mark.cloud();
        let noun = mark.kind.noun();
        if self.cloud_busy.contains(&id)
            || self.archiving.contains(&id)
            || self.cloud_archive.is_some()
        {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(
                "This session is being archived or another cloud archive is open. Nothing was changed",
            )));
            return;
        }
        if self.cloud_workspaces_busy.contains(&target.key()) {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(format!(
                "{} Nothing was changed.",
                WORKSPACE_BUSY
            ))));
            return;
        }
        let groups = group_by_codespace(&[id], &self.cloud_marks, |target, selected| {
            self.other_users_of(target, selected)
        });
        let Some(group) = groups.into_iter().next() else {
            let _ = reply.try_send(Err(ArchiveFailure::untouched("Session not found")));
            return;
        };
        if choice == ArchiveChoice::Delete && group.others > 0 {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(format!(
                "{} other session(s) that are not archived still use the {noun} {}; it was not deleted and nothing was changed. Archive them first, or use cloud \"stop\" or \"keep\".",
                group.others, target.name
            ))));
            return;
        }
        if choice != ArchiveChoice::Keep
            && self.cloud_marks.iter().any(|(other, m)| {
                *other != id
                    && m.cloud() == target
                    && self
                        .remote_marks
                        .get(other)
                        .is_some_and(|(running, _)| *running)
            })
        {
            let _ = reply.try_send(Err(ArchiveFailure::untouched(format!(
                "Another session is running in the {noun} {}; it was left as it was and nothing was changed.",
                target.name
            ))));
            return;
        }
        if let Some(view) = self.sessions.get(&id).cloned() {
            view.update(cx, |v, cx| v.stop_app_session(cx));
        }
        // Delete saves every session of the workspace, so all of them stop taking messages.
        let mut locked_ids = vec![id];
        if choice == ArchiveChoice::Delete {
            locked_ids.extend(
                self.cloud_marks
                    .iter()
                    .filter(|(other, m)| **other != id && m.cloud() == target)
                    .map(|(other, _)| *other),
            );
        }
        self.cloud_busy.extend(locked_ids.iter().copied());
        let key = target.key();
        self.cloud_workspaces_busy.insert(key.clone());
        self.lock_cloud_composer(&locked_ids, true, cx);
        // A copy without the conversation never reaches the stop or delete: the conversion
        // trips `cancel`, which `archive_groups` checks before the workspace call.
        let cancel = Arc::new(AtomicBool::new(false));
        let lost_history = Arc::new(AtomicBool::new(false));
        let inner = self.cloud_convert.clone();
        let convert: ConvertFn = {
            let (cancel, lost) = (cancel.clone(), lost_history.clone());
            Arc::new(move |cloud, session, files_only, progress| {
                let converted = inner(cloud, session, files_only, progress)?;
                if !converted.history_saved && choice != ArchiveChoice::Keep {
                    lost.store(true, Ordering::Relaxed);
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(converted)
            })
        };
        let cloud = self.cloud.clone();
        let work_group = group.clone();
        let work = cx.background_executor().spawn(async move {
            archive_groups(
                &cloud,
                vec![work_group],
                choice,
                backup,
                &convert,
                &cancel,
                &[],
                &[],
                &mut |_| {},
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcomes = work.await;
            let lost = lost_history.load(Ordering::Relaxed);
            let _ = this.update_in(cx, |this, window, cx| {
                this.lock_cloud_composer(&locked_ids, false, cx);
                this.cloud_workspaces_busy.remove(&key);
                let report = outcomes
                    .first()
                    .map(|outcome| archive_report(id, choice, lost, outcome))
                    .unwrap_or_else(|| Err(ArchiveFailure::untouched("Nothing was archived")));
                // Never through the dialog: the user may have one of their own open.
                this.settle_cloud_archive(choice, locked_ids, outcomes, window, cx);
                this.invalidate_chrome(cx);
                cx.notify();
                let _ = reply.try_send(report);
            });
        })
        .detach();
    }
}

/// What the app call answers for one group's outcome.
fn archive_report(
    id: i64,
    choice: ArchiveChoice,
    lost_history: bool,
    outcome: &GroupOutcome,
) -> Result<Value, ArchiveFailure> {
    let group = &outcome.group;
    let noun = group.kind.noun();
    let kept = format!("The {noun} {} was kept.", group.codespace);
    let saved = outcome.converted.iter().find(|c| c.cloud_id == id);
    let copy = saved.map_or(String::new(), |c| {
        format!(" The local copy is archived as session {}.", c.local_id)
    });
    let report = |action: &str, done: bool, warning: Option<String>, archived: bool| {
        let mut value = json!({
            "sessionId": id,
            "archived": archived,
            "cloud": {"kind": noun, "name": group.codespace, "action": action, "done": done},
        });
        if let Some(c) = saved {
            value["archivedSessionId"] = json!(c.local_id);
        }
        let mut warnings: Vec<String> = outcome
            .converted
            .iter()
            .flat_map(|c| c.warnings.iter().cloned())
            .collect();
        warnings.extend(warning);
        if saved.is_some_and(|c| !c.history_saved) {
            warnings.push("The conversation could not be saved; only the code was.".into());
        }
        if !warnings.is_empty() {
            value["warning"] = json!(warnings.join(" "));
        }
        value
    };
    match &outcome.result {
        Ok((Then::Archive, warning)) => Ok(report(choice.word(), true, warning.clone(), true)),
        Ok((Then::Forget, warning)) => {
            let note = saved.is_none().then(|| {
                format!(
                    "The {noun} was deleted and its sessions were dropped from the app.{}",
                    if outcome.converted.is_empty() {
                        " No local copy was needed or made."
                    } else {
                        ""
                    }
                )
            });
            let warning = [warning.clone(), note]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            Ok(report(
                choice.word(),
                true,
                (!warning.is_empty()).then_some(warning),
                saved.is_some(),
            ))
        }
        Err(error) => {
            // What was done decides the wording and the undo: the target counts as archived
            // only when its local copy was saved, whatever stopped the rest.
            let failed = |message: String| {
                Err(ArchiveFailure {
                    message,
                    archived: saved.is_some(),
                })
            };
            let nothing = if saved.is_some() {
                String::new()
            } else {
                " Nothing was archived.".to_string()
            };
            if error == CANCELLED && lost_history {
                failed(format!(
                    "The conversation of the session could not be saved (only its code was), so the {noun} {} was not stopped or deleted.{copy}",
                    group.codespace
                ))
            } else if error == CANCELLED {
                failed(format!("Cancelled.{nothing} {kept}{copy}"))
            } else if error == NEEDS_FILES_ONLY {
                failed(format!(
                    "The conversation of a session in the {noun} could not be reached.{nothing} {kept}{copy} Retry later."
                ))
            } else if error == NEEDS_REVIEW {
                failed(format!(
                    "The {noun} holds files the local copy lacks ({}), so it was not deleted. The user must review them in the app (the session menu offers Delete {noun}), then delete it there. {kept}{copy}",
                    outcome.risky.join("; ")
                ))
            } else if saved.is_some() {
                Ok(report(
                    choice.word(),
                    false,
                    Some(format!(
                        "{error}. The session is archived; the {noun} {} is still there.",
                        group.codespace
                    )),
                    true,
                ))
            } else {
                failed(format!("{error}.{nothing} {kept}"))
            }
        }
    }
}
