//! Builders shared by the ticket model tests. Test only.
#![allow(dead_code)]

use super::tickets::{TicketAssessment, TicketAssignee, TicketRecord, WorkspaceScope};
use crate::contributors::ticket_store::{Criteria, Reasons};

/// A ticket of workspace `w1`, space `s1`, closed on 2026-10-05, nobody assigned.
pub fn ticket(id: &str) -> TicketRecord {
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
        updated_at: "2026-10-05T10:00:00Z".into(),
        done_at: Some("2026-10-05T10:00:00Z".into()),
        status_since: None,
        parent_id: None,
        has_subtasks: false,
        assignees: vec![],
        tags: vec![],
        assessment: None,
        override_level: None,
        override_note: None,
    }
}

/// `ticket(id)` patched by `patch`.
pub fn ticket_with(id: &str, patch: impl FnOnce(&mut TicketRecord)) -> TicketRecord {
    let mut t = ticket(id);
    patch(&mut t);
    t
}

pub fn user(id: &str) -> TicketAssignee {
    named(id, &format!("user-{id}"))
}

pub fn named(id: &str, name: &str) -> TicketAssignee {
    TicketAssignee {
        id: id.into(),
        name: name.into(),
        email: format!("{id}@example.com"),
    }
}

pub fn uniform(score: u8) -> Criteria {
    Criteria {
        ambiguity: score,
        investigation: score,
        scope: score,
        risk: score,
        coordination: score,
    }
}

pub fn assessment_of(criteria: Criteria, reason: &str) -> TicketAssessment {
    TicketAssessment {
        criteria,
        reasons: Reasons {
            ambiguity: format!("{reason}-ambiguity"),
            investigation: format!("{reason}-investigation"),
            scope: format!("{reason}-scope"),
            risk: format!("{reason}-risk"),
            coordination: format!("{reason}-coordination"),
        },
        rubric_version: 1,
        model: "test".into(),
        runner: "api".into(),
        assessed_at: "2026-10-01T00:00:00Z".into(),
    }
}

/// An assessment scoring `score` on every criterion.
pub fn assessment(score: u8) -> TicketAssessment {
    assessment_of(uniform(score), "")
}

pub fn scope(workspace_id: &str) -> WorkspaceScope {
    WorkspaceScope {
        workspace_id: workspace_id.into(),
        name: "Workspace".into(),
        space_ids: None,
        excluded_statuses: vec![],
        delivered_statuses: vec![],
        excluded_members: vec![],
    }
}

pub fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}
