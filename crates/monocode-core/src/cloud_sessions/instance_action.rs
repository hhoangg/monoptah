//! What archiving or deleting a cloud session does to the instance (codespace or Docker
//! sandbox) it runs in, and the guard that keeps one session from pulling the instance out
//! from under another.
//!
//! Several sessions can share one instance ([`CloudSessions::sessions_on`]). The rule, applied
//! in one place so the archive dialog, the delete dialog and the app API cannot disagree:
//!
//! * **Delete** is refused while any other live (non-archived) session uses the instance. It
//!   would destroy that session's workspace. Nothing is deleted.
//! * **Stop** is recoverable (the next send wakes the instance), so it only holds back when
//!   another session is busy (a turn is running, it waits for input, or it is starting). The
//!   instance is then left running and the outcome says so. Idle sessions sharing the
//!   instance do not block it, exactly as the auto-stop treats them.
//!
//! A stop or delete of an instance that no longer exists is a success, see
//! [`CloudSessions::stop_cloud`] and [`CloudSessions::delete_cloud_outcome`].
use super::*;

/// What the user chose to do with the instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceAction {
    Stop,
    Delete,
}

impl InstanceAction {
    /// The words of the app API (`cloud` on `sessions.archive` and `sessions.delete`).
    pub fn api_name(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Delete => "delete",
        }
    }
}

/// What [`plan_instance_action`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstancePlan {
    /// Nothing else uses the instance in a way that matters: do it.
    Proceed,
    /// A stop that must not happen now: these other sessions are busy in the instance.
    LeaveRunning { busy: Vec<String> },
    /// A delete that must not happen: these other live sessions use the instance.
    Refuse { sharing: Vec<String> },
}

/// What [`CloudSessions::apply_instance_action`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceOutcome {
    Stopped,
    /// The instance no longer exists (another process or the user removed it): nothing was
    /// called, and that counts as done.
    AlreadyGone,
    /// The stop was skipped on purpose; the text says why, for the user.
    LeftRunning(String),
    Deleted(DeleteOutcome),
}

fn label(session: &InstanceSession) -> String {
    let title = session
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        "Untitled".into()
    } else {
        title
    }
}

/// Pure decision, `others` being the live sessions of the instance that are NOT being
/// archived or deleted by this action.
pub fn plan_instance_action(action: InstanceAction, others: &[InstanceSession]) -> InstancePlan {
    match action {
        InstanceAction::Delete if others.is_empty() => InstancePlan::Proceed,
        InstanceAction::Delete => InstancePlan::Refuse {
            sharing: others.iter().map(label).collect(),
        },
        InstanceAction::Stop => {
            let busy: Vec<String> = others.iter().filter(|s| s.busy()).map(label).collect();
            if busy.is_empty() {
                InstancePlan::Proceed
            } else {
                InstancePlan::LeaveRunning { busy }
            }
        }
    }
}

/// "Other sessions use this codespace: A, B" for a refusal or a note.
fn names(list: &[String]) -> String {
    list.iter()
        .map(|name| format!("\u{201c}{name}\u{201d}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why a delete was refused, for the user.
pub fn refused_delete_message(cloud: &CloudRef, sharing: &[String]) -> String {
    format!(
        "The {} {} is still used by {}, so it was not deleted. Archive or delete those sessions \
         first, or stop it instead.",
        cloud.noun(),
        cloud.name,
        names(sharing)
    )
}

/// Why a stop was skipped, for the user.
pub fn left_running_message(cloud: &CloudRef, busy: &[String]) -> String {
    format!(
        "The {} {} was left running because {} {} busy in it.",
        cloud.noun(),
        cloud.name,
        names(busy),
        if busy.len() == 1 { "is" } else { "are" }
    )
}

impl CloudSessions {
    /// The live sessions of `cloud` other than `selected`, read from the store now.
    pub fn sessions_sharing(&self, cloud: &CloudRef, selected: &[i64]) -> Vec<InstanceSession> {
        // A local session that was moved into the instance is backed by a Host record there:
        // that record is the selected session itself, not another user of the instance.
        let mut own = selected.to_vec();
        if let Ok(store) = self.store() {
            for id in selected {
                if let Ok(Some((backing, _))) = store.linked_remote_session(*id) {
                    own.push(backing);
                }
            }
        }
        self.sessions_on(cloud)
            .into_iter()
            .filter(|s| !own.contains(&s.id))
            .collect()
    }

    /// Stops or deletes `cloud` on behalf of the sessions `selected`, unless another live
    /// session needs it (see the module docs). The sharing check is made here, from fresh
    /// store reads, whatever the caller showed the user earlier. BLOCKS.
    ///
    /// `Err` is a refused delete (nothing was done) or a real failure of the backend. A missing
    /// instance is never an error.
    pub fn apply_instance_action(
        &self,
        cloud: &CloudRef,
        action: InstanceAction,
        selected: &[i64],
    ) -> Result<InstanceOutcome, String> {
        // Learned earlier in this process (a delete found it missing, a stop did): there is
        // nothing to stop or delete, and calling the backend would only fail.
        if self.marked_gone(cloud) {
            if action == InstanceAction::Delete {
                // The bindings of the sessions that were on it go with it.
                let others = self.sessions_sharing(cloud, selected);
                if others.is_empty() {
                    self.store()?.forget_cloud(cloud)?;
                }
            }
            return Ok(InstanceOutcome::AlreadyGone);
        }
        let others = self.sessions_sharing(cloud, selected);
        match plan_instance_action(action, &others) {
            InstancePlan::Refuse { sharing } => Err(refused_delete_message(cloud, &sharing)),
            InstancePlan::LeaveRunning { busy } => Ok(InstanceOutcome::LeftRunning(
                left_running_message(cloud, &busy),
            )),
            InstancePlan::Proceed => match action {
                InstanceAction::Stop => self.stop_cloud(cloud).map(|()| InstanceOutcome::Stopped),
                InstanceAction::Delete => self
                    .delete_cloud_outcome(cloud)
                    .map(InstanceOutcome::Deleted),
            },
        }
    }
}
