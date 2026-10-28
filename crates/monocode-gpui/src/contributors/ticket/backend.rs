//! What the Ticket tab asks of the outside world (ClickUp, the ticket store,
//! the ticket jobs, the preferences). Every method blocks, so callers run
//! them on the background executor. The trait is the seam the tests use: they
//! never reach ClickUp, `claude` or the user's files.
use crate::runtime::{Database, locked};
use chrono::{DateTime, Local};
use monocode_core::{
    contributors::{
        api::{self, Contributors},
        model::period::PeriodPick,
        ticket_jobs::{TicketEstimate, TicketJobStatus},
        tickets::{
            prefs::{TicketBoardSettings, load_range, load_settings, save_range, save_settings},
            tickets::{AiConfigStatus, TicketRecord, WorkspaceScope},
        },
    },
    inbox::Service,
};
use std::sync::Arc;

/// A ClickUp workspace or space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Named {
    pub id: String,
    pub name: String,
}

/// A change to the saved preferences. Each field is applied on top of what is
/// stored at that moment, so points edited in Settings are never overwritten.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Patch {
    pub scope: Option<Option<WorkspaceScope>>,
    pub percent: Option<f64>,
    pub range: Option<PeriodPick>,
}
impl Patch {
    pub(crate) fn is_empty(&self) -> bool {
        self.scope.is_none() && self.percent.is_none() && self.range.is_none()
    }

    /// `later` on top of `self`: what the two do one after the other.
    pub(crate) fn then(mut self, later: Patch) -> Patch {
        if later.scope.is_some() {
            self.scope = later.scope;
        }
        if later.percent.is_some() {
            self.percent = later.percent;
        }
        if later.range.is_some() {
            self.range = later.range;
        }
        self
    }

    /// The settings with this patch on top of `stored`.
    pub(crate) fn apply(&self, stored: &TicketBoardSettings) -> TicketBoardSettings {
        let mut next = stored.clone();
        if let Some(scope) = &self.scope {
            next.scope = scope.clone();
        }
        if let Some(percent) = self.percent {
            next.percent = percent;
        }
        next
    }
}

pub(crate) trait TicketBackend: Send + Sync {
    /// Whether a ClickUp token is saved.
    fn connected(&self) -> Result<bool, String>;
    fn workspaces(&self) -> Result<Vec<Named>, String>;
    fn spaces(&self, workspace_id: &str) -> Result<Vec<Named>, String>;
    fn ai_config(&self) -> Result<AiConfigStatus, String>;
    fn load(&self, workspace_id: &str, since: &str) -> Result<Vec<TicketRecord>, String>;
    fn estimate(&self, ids: &[String]) -> Result<TicketEstimate, String>;
    /// Queues a fetch; the job id.
    fn fetch(&self, scope: &WorkspaceScope, from: &str, to: &str) -> Result<u64, String>;
    /// Queues an assessment; the job id.
    fn assess(&self, ids: &[String], force: bool) -> Result<u64, String>;
    fn cancel(&self);
    fn status(&self) -> Result<TicketJobStatus, String>;
    fn window_covered(&self, scope: &WorkspaceScope, from: &str, to: &str) -> Result<bool, String>;
    fn set_override(
        &self,
        ticket_id: &str,
        level: Option<&str>,
        note: Option<&str>,
    ) -> Result<(), String>;
    /// The saved board settings and period.
    fn load_prefs(&self, now: &DateTime<Local>) -> (TicketBoardSettings, PeriodPick);
    /// Applies the patch to the stored settings and returns what is stored now.
    fn patch_prefs(&self, patch: &Patch) -> TicketBoardSettings;
}

/// The real thing: the core's ticket API, the native ClickUp client and the
/// shell store's preferences.
pub(crate) struct CoreTicketBackend {
    core: Arc<Contributors>,
    store: Option<Database>,
}
impl CoreTicketBackend {
    pub(crate) fn new(core: Arc<Contributors>, store: Option<Database>) -> Self {
        Self { core, store }
    }

    fn clickup(&self) -> Service {
        Service {
            data_dir: self.core.data_dir().to_path_buf(),
            cwd: std::path::PathBuf::new(),
        }
    }
}

impl TicketBackend for CoreTicketBackend {
    fn connected(&self) -> Result<bool, String> {
        Ok(self.clickup().status("clickup")?.connected)
    }
    fn workspaces(&self) -> Result<Vec<Named>, String> {
        Ok(self
            .clickup()
            .clickup_workspaces(String::new())?
            .into_iter()
            .map(|row| Named {
                id: row.id,
                name: row.name,
            })
            .collect())
    }
    fn spaces(&self, workspace_id: &str) -> Result<Vec<Named>, String> {
        Ok(api::clickup_spaces(&self.core, workspace_id)?
            .into_iter()
            .map(|space| Named {
                id: space.id,
                name: space.name,
            })
            .collect())
    }
    fn ai_config(&self) -> Result<AiConfigStatus, String> {
        api::ai_config_get(&self.core)
    }
    fn load(&self, workspace_id: &str, since: &str) -> Result<Vec<TicketRecord>, String> {
        api::tickets_load(&self.core, &[workspace_id.to_string()], since)
    }
    fn estimate(&self, ids: &[String]) -> Result<TicketEstimate, String> {
        api::tickets_estimate(&self.core, ids)
    }
    fn fetch(&self, scope: &WorkspaceScope, from: &str, to: &str) -> Result<u64, String> {
        api::tickets_fetch(&self.core, vec![scope.link()], from, to)
    }
    fn assess(&self, ids: &[String], force: bool) -> Result<u64, String> {
        api::tickets_assess(&self.core, ids.to_vec(), force)
    }
    fn cancel(&self) {
        api::tickets_cancel(&self.core);
    }
    fn status(&self) -> Result<TicketJobStatus, String> {
        Ok(api::tickets_status(&self.core))
    }
    fn window_covered(&self, scope: &WorkspaceScope, from: &str, to: &str) -> Result<bool, String> {
        api::tickets_window_covered(
            &self.core,
            &scope.workspace_id,
            scope.space_ids.as_deref(),
            from,
            to,
        )
    }
    fn set_override(
        &self,
        ticket_id: &str,
        level: Option<&str>,
        note: Option<&str>,
    ) -> Result<(), String> {
        api::ticket_override(&self.core, ticket_id, level, note)
    }
    fn load_prefs(&self, now: &DateTime<Local>) -> (TicketBoardSettings, PeriodPick) {
        match self.store.as_ref().map(locked) {
            Some(Ok(store)) => (load_settings(&store), load_range(&store, now)),
            _ => (
                monocode_core::contributors::tickets::prefs::read_settings(None),
                monocode_core::contributors::tickets::prefs::read_range(None, now),
            ),
        }
    }
    fn patch_prefs(&self, patch: &Patch) -> TicketBoardSettings {
        let Some(Ok(store)) = self.store.as_ref().map(locked) else {
            return patch.apply(&monocode_core::contributors::tickets::prefs::read_settings(
                None,
            ));
        };
        // Re-read first, so points edited in Settings are not overwritten.
        let next = patch.apply(&load_settings(&store));
        if patch.scope.is_some() || patch.percent.is_some() {
            let _ = save_settings(&store, &next);
        }
        if let Some(range) = &patch.range {
            let _ = save_range(&store, range);
        }
        next
    }
}
