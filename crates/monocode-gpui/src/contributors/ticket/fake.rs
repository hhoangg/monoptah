//! A fake `TicketBackend` and builders for the Ticket tab's tests. Nothing here
//! reaches ClickUp, `claude` or the user's files.
use super::backend::{Named, Patch, TicketBackend};
use crate::contributors::{backend::ColorOverrides, data::Clock};
use chrono::{DateTime, Local, SecondsFormat, TimeZone, Utc};
use monocode_core::contributors::{
    model::period::{PeriodKind, PeriodPick},
    ticket_jobs::{TicketEstimate, TicketJobEvent, TicketJobStatus},
    tickets::{
        prefs::TicketBoardSettings,
        tickets::{
            AiConfigStatus, Criteria, DEFAULT_TICKET_POINTS, Reasons, TicketAssessment,
            TicketAssignee, TicketRecord, WorkspaceScope,
        },
    },
};
use std::{
    cell::Cell,
    collections::VecDeque,
    rc::Rc,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Call {
    Connected,
    Workspaces,
    Spaces(String),
    AiConfig,
    Load {
        workspace: String,
        since: String,
    },
    Estimate(Vec<String>),
    Fetch {
        scope: WorkspaceScope,
        from: String,
        to: String,
    },
    Assess {
        ids: Vec<String>,
        force: bool,
    },
    Cancel,
    Status,
    Covered {
        workspace: String,
        spaces: Option<Vec<String>>,
        from: String,
        to: String,
    },
    Override {
        id: String,
        level: Option<String>,
        note: Option<String>,
    },
    Patch(Patch),
    /// The previous period read from the store: never a fetch.
    Previous {
        workspace: String,
        from: String,
        to: String,
    },
    /// A year of tickets read from the store: never a fetch.
    History {
        workspace: String,
        from: String,
        to: String,
    },
    SaveChartKind(String),
    SaveColors {
        workspace: String,
        colors: ColorOverrides,
    },
}

/// Answers in order, then the default.
pub(super) struct Script<T: Clone> {
    pub(super) once: VecDeque<T>,
    pub(super) default: T,
}
impl<T: Clone> Script<T> {
    fn new(default: T) -> Self {
        Self {
            once: VecDeque::new(),
            default,
        }
    }
    fn next(&mut self) -> T {
        self.once
            .pop_front()
            .unwrap_or_else(|| self.default.clone())
    }
}

pub(super) type OnFetch = Arc<dyn Fn(u64) + Send + Sync>;

pub(super) struct State {
    pub(super) calls: Vec<Call>,
    pub(super) connected: Script<Result<bool, String>>,
    pub(super) workspaces: Script<Result<Vec<Named>, String>>,
    pub(super) spaces: Script<Result<Vec<Named>, String>>,
    pub(super) ai: Script<Result<AiConfigStatus, String>>,
    pub(super) load: Script<Result<Vec<TicketRecord>, String>>,
    pub(super) estimate: Script<Result<TicketEstimate, String>>,
    pub(super) fetch: Script<Result<u64, String>>,
    pub(super) assess: Script<Result<u64, String>>,
    pub(super) status: Script<Result<TicketJobStatus, String>>,
    pub(super) covered: Script<Result<bool, String>>,
    /// What the store has of the previous period; `None` is "not covered".
    pub(super) previous: Script<Result<Option<Vec<TicketRecord>>, String>>,
    pub(super) overridden: Script<Result<(), String>>,
    /// Runs inside `fetch` with the job id about to be answered.
    pub(super) on_fetch: Option<OnFetch>,
    pub(super) stored: TicketBoardSettings,
    pub(super) range: PeriodPick,
    /// Whether the store holds the whole year the member page reads.
    pub(super) history_covered: bool,
    pub(super) chart_kind: Option<String>,
    pub(super) colors: std::collections::BTreeMap<String, ColorOverrides>,
}

pub(super) struct TicketFake(Mutex<State>);

pub(super) fn named(id: &str, name: &str) -> Named {
    Named {
        id: id.into(),
        name: name.into(),
    }
}

pub(super) fn scope() -> WorkspaceScope {
    WorkspaceScope {
        workspace_id: "w1".into(),
        name: "Acme".into(),
        space_ids: None,
        excluded_statuses: vec![],
        delivered_statuses: vec![],
        excluded_members: vec![],
    }
}

pub(super) fn settings(scope: Option<WorkspaceScope>, percent: f64) -> TicketBoardSettings {
    TicketBoardSettings {
        scope,
        points: DEFAULT_TICKET_POINTS,
        percent,
    }
}

pub(super) fn idle() -> TicketJobStatus {
    TicketJobStatus {
        event: None,
        pending_fetches: 0,
    }
}

pub(super) fn ai() -> AiConfigStatus {
    AiConfigStatus {
        runner: "claude-code".into(),
        model: "sonnet".into(),
        has_api_key: false,
    }
}

/// "Today" of the tests: 2026-10-17 at noon.
pub(super) fn now() -> DateTime<Local> {
    Local.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap()
}

/// A moment on a local day of 2026, as the ISO text the store keeps.
pub(super) fn at(month: u32, day: u32, hour: u32) -> String {
    Local
        .with_ymd_and_hms(2026, month, day, hour, 0, 0)
        .unwrap()
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(super) fn user(id: &str, name: &str) -> TicketAssignee {
    TicketAssignee {
        id: id.into(),
        name: name.into(),
        email: format!("{id}@example.com"),
    }
}

pub(super) fn assessment(score: u8) -> TicketAssessment {
    TicketAssessment {
        criteria: Criteria {
            // The criteria sum picks the level: 2 is S, 6 is L.
            ambiguity: score,
            investigation: 0,
            scope: 0,
            risk: 0,
            coordination: 0,
        },
        reasons: Reasons {
            ambiguity: "Unclear brief.".into(),
            investigation: "Some digging.".into(),
            scope: "Two modules.".into(),
            risk: "Low.".into(),
            coordination: "Solo.".into(),
        },
        rubric_version: 1,
        model: "sonnet".into(),
        runner: "claude-code".into(),
        assessed_at: "2026-10-02T00:00:00Z".into(),
    }
}

/// A ticket of workspace `w1`, closed on 2026-10-05, Alpha on it.
pub(super) fn ticket(id: &str) -> TicketRecord {
    TicketRecord {
        source: "clickup".into(),
        id: id.into(),
        custom_id: None,
        title: format!("Ticket {id}"),
        url: format!("https://app.clickup.com/t/{id}"),
        workspace_id: "w1".into(),
        space_id: "s1".into(),
        status: "closed".into(),
        status_type: "closed".into(),
        updated_at: at(10, 5, 10),
        done_at: Some(at(10, 5, 10)),
        status_since: None,
        parent_id: None,
        has_subtasks: false,
        assignees: vec![user("a", "Alpha")],
        tags: vec![],
        assessment: None,
        override_level: None,
        override_note: None,
    }
}

pub(super) fn ticket_with(id: &str, patch: impl FnOnce(&mut TicketRecord)) -> TicketRecord {
    let mut ticket = ticket(id);
    patch(&mut ticket);
    ticket
}

/// A delivered ticket and one in a working status, both Alpha's.
pub(super) fn stored_tickets() -> Vec<TicketRecord> {
    vec![
        ticket("done"),
        ticket_with("testing", |t| {
            t.status = "ready for testing".into();
            t.status_type = "custom".into();
            t.done_at = None;
        }),
    ]
}

impl TicketFake {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(State {
            calls: Vec::new(),
            connected: Script::new(Ok(true)),
            workspaces: Script::new(Ok(vec![named("w1", "Acme"), named("w2", "Beta Corp")])),
            spaces: Script::new(Ok(vec![named("s1", "Engineering"), named("s2", "Support")])),
            ai: Script::new(Ok(ai())),
            load: Script::new(Ok(stored_tickets())),
            estimate: Script::new(Ok(TicketEstimate {
                tickets: 1,
                input_tokens: 12345,
            })),
            fetch: Script::new(Ok(1)),
            assess: Script::new(Ok(1)),
            status: Script::new(Ok(idle())),
            covered: Script::new(Ok(false)),
            previous: Script::new(Ok(None)),
            overridden: Script::new(Ok(())),
            on_fetch: None,
            stored: settings(Some(scope()), 100.),
            range: PeriodPick {
                kind: PeriodKind::Month,
                id: None,
            },
            history_covered: true,
            chart_kind: None,
            colors: Default::default(),
        })))
    }

    pub(super) fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }

    pub(super) fn calls(&self) -> Vec<Call> {
        self.with(|s| s.calls.clone())
    }

    pub(super) fn count(&self, pred: impl Fn(&Call) -> bool) -> usize {
        self.with(|s| s.calls.iter().filter(|c| pred(c)).count())
    }

    pub(super) fn fetches(&self) -> usize {
        self.count(|c| matches!(c, Call::Fetch { .. }))
    }

    pub(super) fn loads(&self) -> usize {
        self.count(|c| matches!(c, Call::Load { .. }))
    }

    pub(super) fn previous_reads(&self) -> usize {
        self.count(|c| matches!(c, Call::Previous { .. }))
    }

    pub(super) fn estimates(&self) -> Vec<Vec<String>> {
        self.with(|s| {
            s.calls
                .iter()
                .filter_map(|c| match c {
                    Call::Estimate(ids) => Some(ids.clone()),
                    _ => None,
                })
                .collect()
        })
    }

    pub(super) fn clear_calls(&self) {
        self.with(|s| s.calls.clear());
    }

    pub(super) fn stored(&self) -> TicketBoardSettings {
        self.with(|s| s.stored.clone())
    }
}

impl TicketBackend for TicketFake {
    fn connected(&self) -> Result<bool, String> {
        self.with(|s| {
            s.calls.push(Call::Connected);
            s.connected.next()
        })
    }
    fn workspaces(&self) -> Result<Vec<Named>, String> {
        self.with(|s| {
            s.calls.push(Call::Workspaces);
            s.workspaces.next()
        })
    }
    fn spaces(&self, workspace_id: &str) -> Result<Vec<Named>, String> {
        self.with(|s| {
            s.calls.push(Call::Spaces(workspace_id.into()));
            s.spaces.next()
        })
    }
    fn ai_config(&self) -> Result<AiConfigStatus, String> {
        self.with(|s| {
            s.calls.push(Call::AiConfig);
            s.ai.next()
        })
    }
    fn load(&self, workspace_id: &str, since: &str) -> Result<Vec<TicketRecord>, String> {
        self.with(|s| {
            s.calls.push(Call::Load {
                workspace: workspace_id.into(),
                since: since.into(),
            });
            s.load.next()
        })
    }
    fn estimate(&self, ids: &[String]) -> Result<TicketEstimate, String> {
        self.with(|s| {
            s.calls.push(Call::Estimate(ids.to_vec()));
            s.estimate.next()
        })
    }
    fn fetch(&self, scope: &WorkspaceScope, from: &str, to: &str) -> Result<u64, String> {
        let (answer, hook) = self.with(|s| {
            s.calls.push(Call::Fetch {
                scope: scope.clone(),
                from: from.into(),
                to: to.into(),
            });
            (s.fetch.next(), s.on_fetch.clone())
        });
        if let (Ok(id), Some(hook)) = (&answer, hook) {
            hook(*id);
        }
        answer
    }
    fn assess(&self, ids: &[String], force: bool) -> Result<u64, String> {
        self.with(|s| {
            s.calls.push(Call::Assess {
                ids: ids.to_vec(),
                force,
            });
            s.assess.next()
        })
    }
    fn cancel(&self) {
        self.with(|s| s.calls.push(Call::Cancel));
    }
    fn status(&self) -> Result<TicketJobStatus, String> {
        self.with(|s| {
            s.calls.push(Call::Status);
            s.status.next()
        })
    }
    fn window_covered(&self, scope: &WorkspaceScope, from: &str, to: &str) -> Result<bool, String> {
        self.with(|s| {
            s.calls.push(Call::Covered {
                workspace: scope.workspace_id.clone(),
                spaces: scope.space_ids.clone(),
                from: from.into(),
                to: to.into(),
            });
            s.covered.next()
        })
    }
    fn previous(
        &self,
        scope: &WorkspaceScope,
        from: &str,
        to: &str,
    ) -> Result<Option<Vec<TicketRecord>>, String> {
        self.with(|s| {
            s.calls.push(Call::Previous {
                workspace: scope.workspace_id.clone(),
                from: from.into(),
                to: to.into(),
            });
            s.previous.next()
        })
    }
    fn history(
        &self,
        scope: &WorkspaceScope,
        from: &str,
        to: &str,
    ) -> Result<(Vec<TicketRecord>, bool), String> {
        self.with(|s| {
            s.calls.push(Call::History {
                workspace: scope.workspace_id.clone(),
                from: from.into(),
                to: to.into(),
            });
            // What the store holds is what a load answers.
            s.load
                .default
                .clone()
                .map(|tickets| (tickets, s.history_covered))
        })
    }
    fn set_override(
        &self,
        ticket_id: &str,
        level: Option<&str>,
        note: Option<&str>,
    ) -> Result<(), String> {
        self.with(|s| {
            s.calls.push(Call::Override {
                id: ticket_id.into(),
                level: level.map(String::from),
                note: note.map(String::from),
            });
            let answer = s.overridden.next();
            // The store keeps what was set, so the next read shows it.
            if answer.is_ok()
                && let Ok(list) = &mut s.load.default
            {
                for ticket in list.iter_mut().filter(|t| t.id == ticket_id) {
                    ticket.override_level = level.map(String::from);
                    ticket.override_note = level.and(note).map(String::from);
                }
            }
            answer
        })
    }
    fn load_prefs(&self, _: &DateTime<Local>) -> (TicketBoardSettings, PeriodPick) {
        self.with(|s| (s.stored.clone(), s.range.clone()))
    }
    fn patch_prefs(&self, patch: &Patch) -> TicketBoardSettings {
        self.with(|s| {
            s.calls.push(Call::Patch(patch.clone()));
            s.stored = patch.apply(&s.stored);
            if let Some(range) = &patch.range {
                s.range = range.clone();
            }
            s.stored.clone()
        })
    }
    fn load_chart_kind(&self) -> Option<String> {
        self.with(|s| s.chart_kind.clone())
    }
    fn save_chart_kind(&self, kind: &str) {
        self.with(|s| {
            s.calls.push(Call::SaveChartKind(kind.into()));
            s.chart_kind = Some(kind.into());
        });
    }
    fn load_colors(&self, workspace_id: &str) -> ColorOverrides {
        self.with(|s| s.colors.get(workspace_id).cloned().unwrap_or_default())
    }
    fn save_colors(&self, workspace_id: &str, colors: &ColorOverrides) {
        self.with(|s| {
            s.calls.push(Call::SaveColors {
                workspace: workspace_id.into(),
                colors: colors.clone(),
            });
            s.colors.insert(workspace_id.into(), colors.clone());
        });
    }
}

/// A clock the tests can move.
pub(super) fn clock(cell: &Rc<Cell<DateTime<Local>>>) -> Clock {
    let source = cell.clone();
    Rc::new(move || source.get())
}

/// A job event for the tests.
pub(super) fn event(kind: &str, state: &str, job_id: u64, pending: u32) -> TicketJobEvent {
    TicketJobEvent {
        kind: kind.into(),
        state: state.into(),
        done: 0,
        total: 1,
        failed: 0,
        error: None,
        job_id,
        pending_fetches: pending,
    }
}
