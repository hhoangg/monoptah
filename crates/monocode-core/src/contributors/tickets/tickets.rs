//! Levels, scope rules and the delivered/active classification of tickets.
//! Port of `ticket-contributors/model/tickets.ts`. The ticket is the backend
//! `TicketRecord`; its level strings (`override_level`) stay text because the
//! store keeps them as text.

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use std::ops::{Index, IndexMut};

pub use crate::contributors::assessor::AiConfigStatus;
use crate::contributors::clickup_tickets::TicketLink;
use crate::contributors::model::{js_trim, serialize_number};
pub use crate::contributors::ticket_store::{
    Criteria, Reasons, StoredAssessment as TicketAssessment, TicketAssignee, TicketRecord,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    Xs,
    S,
    M,
    L,
    Xl,
}

pub const LEVELS: [Level; 5] = [Level::Xs, Level::S, Level::M, Level::L, Level::Xl];

impl Level {
    /// `XS`, `S`, `M`, `L` or `XL`: the text the store and the saved settings use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Xs => "XS",
            Self::S => "S",
            Self::M => "M",
            Self::L => "L",
            Self::Xl => "XL",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        LEVELS.into_iter().find(|level| level.as_str() == text)
    }

    /// Position in `LEVELS`, smallest first.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// The five rubric criteria, in the order ties are broken.
pub const CRITERIA: [&str; 5] = [
    "ambiguity",
    "investigation",
    "scope",
    "risk",
    "coordination",
];

impl Criteria {
    pub fn from_scores(scores: [u8; 5]) -> Self {
        Self {
            ambiguity: scores[0],
            investigation: scores[1],
            scope: scores[2],
            risk: scores[3],
            coordination: scores[4],
        }
    }

    /// The score of the criterion at `index` of `CRITERIA`.
    pub fn get(&self, index: usize) -> u8 {
        [
            self.ambiguity,
            self.investigation,
            self.scope,
            self.risk,
            self.coordination,
        ][index]
    }
}

impl Reasons {
    /// The reason of the criterion at `index` of `CRITERIA`.
    pub fn get(&self, index: usize) -> &str {
        [
            &self.ambiguity,
            &self.investigation,
            &self.scope,
            &self.risk,
            &self.coordination,
        ][index]
    }
}

/// One value per level (points of a level, tickets at a level).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ByLevel<T>(pub [T; 5]);

impl<T> Index<Level> for ByLevel<T> {
    type Output = T;
    fn index(&self, level: Level) -> &T {
        &self.0[level.index()]
    }
}

impl<T> IndexMut<Level> for ByLevel<T> {
    fn index_mut(&mut self, level: Level) -> &mut T {
        &mut self.0[level.index()]
    }
}

/// Points per level; serializes as `{"XS":3,...}` with JS number text.
pub type Points = ByLevel<f64>;
/// Tickets per level.
pub type LevelCounts = ByLevel<usize>;

struct Number<'a>(&'a f64);

impl Serialize for Number<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_number(self.0, serializer)
    }
}

impl Serialize for ByLevel<f64> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(5))?;
        for level in LEVELS {
            map.serialize_entry(level.as_str(), &Number(&self[level]))?;
        }
        map.end()
    }
}

impl Serialize for ByLevel<usize> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(5))?;
        for level in LEVELS {
            map.serialize_entry(level.as_str(), &self[level])?;
        }
        map.end()
    }
}

pub const DEFAULT_TICKET_POINTS: Points = ByLevel([3.0, 8.0, 16.0, 32.0, 48.0]);

/// A member left off the board; the name is kept so the list reads without the member's tickets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExcludedMember {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceScope {
    pub workspace_id: String,
    pub name: String,
    /// `None` means every space of the workspace.
    pub space_ids: Option<Vec<String>>,
    pub excluded_statuses: Vec<String>,
    /// Status names that count as delivered work on top of the default (every
    /// status of type done or closed). A done or closed status is taken out
    /// with `excluded_statuses`, not from here.
    pub delivered_statuses: Vec<String>,
    /// ClickUp user ids left off the board. An excluded member counts as if
    /// they were not an assignee (see `included_assignees`).
    pub excluded_members: Vec<ExcludedMember>,
}

impl WorkspaceScope {
    /// What a fetch is asked for: the workspace and its chosen spaces.
    pub fn link(&self) -> TicketLink {
        TicketLink {
            workspace_id: self.workspace_id.clone(),
            space_ids: self.space_ids.clone(),
        }
    }
}

/// Sum of the criteria scores picks the level: two points per step, capped at XL.
pub fn level_of(criteria: &Criteria) -> Level {
    let sum: usize = (0..CRITERIA.len())
        .map(|i| usize::from(criteria.get(i)))
        .sum();
    LEVELS[(sum / 2).min(LEVELS.len() - 1)]
}

/// The manual override wins, then the assessment, else the ticket is unassessed.
pub fn effective_level(t: &TicketRecord) -> Option<Level> {
    if let Some(level) = t.override_level.as_deref().and_then(Level::parse) {
        return Some(level);
    }
    t.assessment.as_ref().map(|a| level_of(&a.criteria))
}

/// Status names compare case-insensitively and trimmed.
pub(crate) fn status_key(name: &str) -> String {
    js_trim(name).to_lowercase()
}

fn is_finished_type(status_type: &str) -> bool {
    status_type == "done" || status_type == "closed"
}

/// The assignees that count on the board: the ticket's own minus the excluded
/// members. The one place that rule lives.
pub fn included_assignees(t: &TicketRecord, scope: &WorkspaceScope) -> Vec<TicketAssignee> {
    if scope.excluded_members.is_empty() {
        return t.assignees.clone();
    }
    t.assignees
        .iter()
        .filter(|a| !scope.excluded_members.iter().any(|m| m.id == a.id))
        .cloned()
        .collect()
}

/// The ticket as the board sees it: only its included assignees.
pub fn with_included_assignees(t: &TicketRecord, scope: &WorkspaceScope) -> TicketRecord {
    let mut seen = t.clone();
    seen.assignees = included_assignees(t, scope);
    seen
}

/// True when the ticket had assignees and every one is excluded, so it drops
/// out of the board. A ticket without any assignee is never dropped.
fn dropped_by_exclusion(t: &TicketRecord, scope: &WorkspaceScope) -> bool {
    !t.assignees.is_empty() && included_assignees(t, scope).is_empty()
}

/// True when the ticket belongs to the scope's workspace/space, is not in an
/// excluded status and is not left with only excluded assignees.
pub fn in_scope(t: &TicketRecord, scope: &WorkspaceScope) -> bool {
    if scope.workspace_id != t.workspace_id {
        return false;
    }
    if let Some(spaces) = &scope.space_ids {
        if !spaces.contains(&t.space_id) {
            return false;
        }
    }
    if dropped_by_exclusion(t, scope) {
        return false;
    }
    let status = status_key(&t.status);
    !scope
        .excluded_statuses
        .iter()
        .any(|s| status_key(s) == status)
}

/// The status rule alone: a done or closed type, or a name chosen as delivered.
fn has_delivered_status(t: &TicketRecord, scope: &WorkspaceScope) -> bool {
    if is_finished_type(&t.status_type) {
        return true;
    }
    let status = status_key(&t.status);
    scope
        .delivered_statuses
        .iter()
        .any(|s| status_key(s) == status)
}

/// Delivered work: in scope (so not excluded, which wins over delivered), in a
/// delivered status, and not a parent whose work lives in its subtasks.
pub fn is_delivered(t: &TicketRecord, scope: &WorkspaceScope) -> bool {
    in_scope(t, scope) && !t.has_subtasks && has_delivered_status(t, scope)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveredAt<'a> {
    pub at: &'a str,
    pub approximate: bool,
}

/// When the ticket was delivered, by preference: its done date (done and closed
/// types), the date it entered its status, its last update. Only the last one
/// is approximate.
pub fn delivered_at(t: &TicketRecord) -> DeliveredAt<'_> {
    if is_finished_type(&t.status_type) {
        if let Some(done) = t.done_at.as_deref().filter(|d| !d.is_empty()) {
            return DeliveredAt {
                at: done,
                approximate: false,
            };
        }
    }
    if let Some(since) = t.status_since.as_deref().filter(|s| !s.is_empty()) {
        return DeliveredAt {
            at: since,
            approximate: false,
        };
    }
    DeliveredAt {
        at: &t.updated_at,
        approximate: true,
    }
}

/// Work in progress: a custom status that is neither delivered nor excluded.
/// A parent with subtasks is left out like it is for delivered work, so the
/// same work is not counted twice. Anchored by `updated_at`, never scored.
pub fn is_active(t: &TicketRecord, scope: &WorkspaceScope) -> bool {
    in_scope(t, scope)
        && !t.has_subtasks
        && t.status_type == "custom"
        && !has_delivered_status(t, scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::ticket_store::Criteria;
    use crate::contributors::tickets::test_fixtures::{
        assessment_of, named, scope as scope_of, strings, ticket as ticket_of, ticket_with,
    };

    fn criteria_with_sum(sum: u8) -> Criteria {
        let mut scores = [0u8; 5];
        let mut left = sum;
        for score in &mut scores {
            let take = left.min(2);
            *score = take;
            left -= take;
        }
        Criteria::from_scores(scores)
    }

    /// The `tickets.test.ts` defaults: workspace 9001, status done.
    fn ticket(patch: impl FnOnce(&mut TicketRecord)) -> TicketRecord {
        ticket_with("t1", |t| {
            t.workspace_id = "9001".into();
            t.status = "done".into();
            t.status_type = "done".into();
            t.updated_at = "2026-10-01T00:00:00Z".into();
            t.done_at = Some("2026-10-01T00:00:00Z".into());
            patch(t);
        })
    }

    fn scope(patch: impl FnOnce(&mut WorkspaceScope)) -> WorkspaceScope {
        let mut s = scope_of("9001");
        s.name = "Acme".into();
        patch(&mut s);
        s
    }

    fn plain() -> TicketRecord {
        ticket(|_| {})
    }

    fn none() -> WorkspaceScope {
        scope(|_| {})
    }

    #[test]
    fn level_of_maps_sums_to_levels_at_every_boundary() {
        let expected = [
            (0, Level::Xs),
            (1, Level::Xs),
            (2, Level::S),
            (3, Level::S),
            (4, Level::M),
            (5, Level::M),
            (6, Level::L),
            (7, Level::L),
            (8, Level::Xl),
            (10, Level::Xl),
        ];
        for (sum, level) in expected {
            assert_eq!(level_of(&criteria_with_sum(sum)), level, "sum {sum}");
        }
    }

    #[test]
    fn effective_level_prefers_the_override_then_the_assessment_else_none() {
        let assessment = assessment_of(criteria_with_sum(6), "");
        assert_eq!(effective_level(&plain()), None);
        let with = ticket(|t| t.assessment = Some(assessment.clone()));
        assert_eq!(effective_level(&with), Some(Level::L));
        let both = ticket(|t| {
            t.assessment = Some(assessment.clone());
            t.override_level = Some("XS".into());
        });
        assert_eq!(effective_level(&both), Some(Level::Xs));
        let only = ticket(|t| t.override_level = Some("M".into()));
        assert_eq!(effective_level(&only), Some(Level::M));
    }

    #[test]
    fn in_scope_rejects_a_ticket_from_another_workspace() {
        assert!(!in_scope(
            &ticket(|t| t.workspace_id = "other".into()),
            &none()
        ));
        assert!(in_scope(&plain(), &none()));
    }

    #[test]
    fn in_scope_honours_space_ids_when_set_and_ignores_them_when_null() {
        let spaces = |ids: Option<&[&str]>| scope(|s| s.space_ids = ids.map(strings));
        let in_space = |id: &str| ticket(|t| t.space_id = id.into());
        assert!(in_scope(&in_space("s2"), &spaces(None)));
        assert!(in_scope(&in_space("s1"), &spaces(Some(&["s1"]))));
        assert!(!in_scope(&in_space("s2"), &spaces(Some(&["s1"]))));
        assert!(!in_scope(&in_space("s1"), &spaces(Some(&[]))));
    }

    #[test]
    fn in_scope_rejects_excluded_statuses_case_insensitively() {
        let s = scope(|s| s.excluded_statuses = strings(&["cancelled"]));
        let with = |status: &str| ticket(|t| t.status = status.into());
        assert!(!in_scope(&with("Cancelled"), &s));
        assert!(!in_scope(&with(" CANCELLED "), &s));
        assert!(in_scope(&with("done"), &s));
    }

    #[test]
    fn in_scope_does_not_look_at_parents_subtasks_are_not_this_functions_job() {
        let sub = ticket(|t| {
            t.id = "s".into();
            t.parent_id = Some("p".into());
        });
        assert!(in_scope(&sub, &none()));
        assert!(in_scope(&ticket(|t| t.has_subtasks = true), &none()));
    }

    #[test]
    fn delivered_at_prefers_the_done_date_of_a_done_or_closed_ticket_over_later_edits() {
        let t = ticket(|t| {
            t.done_at = Some("2026-09-20T00:00:00Z".into());
            t.status_since = Some("2026-09-25T00:00:00Z".into());
            t.updated_at = "2026-10-30T00:00:00Z".into();
        });
        assert_eq!(
            delivered_at(&t),
            DeliveredAt {
                at: "2026-09-20T00:00:00Z",
                approximate: false
            }
        );
        let closed = TicketRecord {
            status_type: "closed".into(),
            ..t
        };
        assert_eq!(delivered_at(&closed).at, "2026-09-20T00:00:00Z");
    }

    #[test]
    fn delivered_at_uses_status_since_for_a_custom_status_exact() {
        let t = ticket(|t| {
            t.status = "ready for testing".into();
            t.status_type = "custom".into();
            t.done_at = None;
            t.status_since = Some("2026-09-25T00:00:00Z".into());
            t.updated_at = "2026-10-30T00:00:00Z".into();
        });
        assert_eq!(
            delivered_at(&t),
            DeliveredAt {
                at: "2026-09-25T00:00:00Z",
                approximate: false
            }
        );
    }

    #[test]
    fn delivered_at_falls_back_to_updated_at_flagged_approximate() {
        let custom = ticket(|t| {
            t.status_type = "custom".into();
            t.done_at = None;
            t.status_since = None;
            t.updated_at = "2026-10-30T00:00:00Z".into();
        });
        assert_eq!(
            delivered_at(&custom),
            DeliveredAt {
                at: "2026-10-30T00:00:00Z",
                approximate: true
            }
        );
        // A done ticket without a done date is no better.
        let done = TicketRecord {
            status_type: "done".into(),
            ..custom
        };
        assert!(delivered_at(&done).approximate);
    }

    #[test]
    fn delivered_at_ignores_a_stray_done_date_on_a_custom_status() {
        let t = ticket(|t| {
            t.status_type = "custom".into();
            t.done_at = Some("2026-01-01T00:00:00Z".into());
            t.status_since = None;
        });
        assert_eq!(delivered_at(&t).at, t.updated_at);
    }

    #[test]
    fn is_delivered_defaults_to_every_done_or_closed_status_type() {
        let of = |kind: &str| ticket(|t| t.status_type = kind.into());
        assert!(is_delivered(&of("done"), &none()));
        assert!(is_delivered(&of("closed"), &none()));
        assert!(!is_delivered(&of("custom"), &none()));
        assert!(!is_delivered(&of("open"), &none()));
    }

    #[test]
    fn is_delivered_adds_the_chosen_names_to_the_done_and_closed_types_case_insensitively() {
        let s = scope(|s| s.delivered_statuses = strings(&[" Ready For Testing "]));
        let custom = ticket(|t| {
            t.status = "ready for testing".into();
            t.status_type = "custom".into();
        });
        assert!(is_delivered(&custom, &s));
        // The default still counts next to the chosen names.
        assert!(is_delivered(&ticket(|t| t.status = "done".into()), &s));
        assert!(!is_delivered(
            &custom,
            &scope(|s| s.delivered_statuses = vec![])
        ));
    }

    #[test]
    fn is_delivered_counts_a_done_status_the_checklist_never_listed() {
        // Edited while only "ready for testing" was known; "brand new" shows up later.
        let s = scope(|s| s.delivered_statuses = strings(&["ready for testing"]));
        let t = ticket(|t| {
            t.status = "brand new".into();
            t.status_type = "closed".into();
        });
        assert!(is_delivered(&t, &s));
    }

    #[test]
    fn is_delivered_lets_an_excluded_status_win_over_a_delivered_one() {
        let s = scope(|s| {
            s.delivered_statuses = strings(&["ready for testing"]);
            s.excluded_statuses = strings(&["READY FOR TESTING"]);
        });
        let t = ticket(|t| {
            t.status = "ready for testing".into();
            t.status_type = "custom".into();
        });
        assert!(!is_delivered(&t, &s));
        assert!(!is_delivered(
            &plain(),
            &scope(|s| s.excluded_statuses = strings(&["done"]))
        ));
    }

    #[test]
    fn is_delivered_never_counts_a_parent_with_subtasks_but_counts_the_subtask() {
        assert!(!is_delivered(&ticket(|t| t.has_subtasks = true), &none()));
        assert!(is_delivered(
            &ticket(|t| t.parent_id = Some("p".into())),
            &none()
        ));
    }

    #[test]
    fn is_delivered_respects_the_workspace_and_spaces() {
        assert!(!is_delivered(
            &ticket(|t| t.workspace_id = "other".into()),
            &none()
        ));
        assert!(!is_delivered(
            &ticket(|t| t.space_id = "s2".into()),
            &scope(|s| s.space_ids = Some(strings(&["s1"])))
        ));
    }

    fn working(patch: impl FnOnce(&mut TicketRecord)) -> TicketRecord {
        ticket(|t| {
            t.status = "in progress".into();
            t.status_type = "custom".into();
            patch(t);
        })
    }

    #[test]
    fn is_active_is_a_custom_status_that_is_not_delivered() {
        assert!(is_active(&working(|_| {}), &none()));
        assert!(!is_active(
            &ticket(|t| t.status_type = "open".into()),
            &none()
        ));
        assert!(!is_active(
            &ticket(|t| t.status_type = "done".into()),
            &none()
        ));
    }

    #[test]
    fn is_active_is_not_active_once_the_status_is_chosen_as_delivered() {
        let s = scope(|s| s.delivered_statuses = strings(&["in progress"]));
        assert!(!is_active(&working(|_| {}), &s));
        assert!(is_delivered(&working(|_| {}), &s));
    }

    #[test]
    fn is_active_a_done_status_stays_delivered_so_it_is_not_active_next_to_chosen_names() {
        let s = scope(|s| s.delivered_statuses = strings(&["ready for testing"]));
        assert!(is_delivered(&plain(), &s));
        assert!(!is_active(&plain(), &s));
    }

    #[test]
    fn is_active_an_excluded_done_status_is_neither_delivered_nor_active() {
        let s = scope(|s| s.excluded_statuses = strings(&["done"]));
        assert!(!is_delivered(&plain(), &s));
        assert!(!is_active(&plain(), &s));
    }

    #[test]
    fn is_active_is_not_active_when_excluded_out_of_scope_or_a_parent_with_subtasks() {
        assert!(!is_active(
            &working(|_| {}),
            &scope(|s| s.excluded_statuses = strings(&["In Progress"]))
        ));
        assert!(!is_active(
            &working(|t| t.workspace_id = "other".into()),
            &none()
        ));
        assert!(!is_active(&working(|t| t.has_subtasks = true), &none()));
        assert!(is_active(
            &working(|t| t.parent_id = Some("p".into())),
            &none()
        ));
    }

    fn person(id: &str) -> TicketAssignee {
        named(id, &format!("User {id}"))
    }

    fn excluding_two() -> WorkspaceScope {
        scope(|s| {
            s.excluded_members = vec![ExcludedMember {
                id: "2".into(),
                name: "Two".into(),
            }]
        })
    }

    #[test]
    fn excluded_members_included_assignees_drops_the_excluded_members() {
        let t = ticket(|t| t.assignees = vec![person("1"), person("2")]);
        let ids: Vec<String> = included_assignees(&t, &excluding_two())
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, ["1"]);
        assert_eq!(included_assignees(&t, &none()), t.assignees);
    }

    #[test]
    fn excluded_members_a_ticket_whose_assignees_are_all_excluded_is_out_of_scope() {
        let s = excluding_two();
        let theirs = ticket(|t| t.assignees = vec![person("2")]);
        assert!(!in_scope(&theirs, &s));
        assert!(!is_delivered(&theirs, &s));
        assert!(!is_active(
            &working(|t| t.assignees = vec![person("2")]),
            &s
        ));
    }

    #[test]
    fn excluded_members_a_ticket_shared_with_an_included_member_or_with_nobody_stays() {
        let s = excluding_two();
        assert!(is_delivered(
            &ticket(|t| t.assignees = vec![person("1"), person("2")]),
            &s
        ));
        assert!(is_delivered(&ticket(|t| t.assignees = vec![]), &s));
        assert!(is_active(&working(|t| t.assignees = vec![]), &s));
    }

    // not in the TS suite: the Rust-only helpers

    #[test]
    fn levels_parse_and_print_their_names() {
        for (i, level) in LEVELS.iter().enumerate() {
            assert_eq!(Level::parse(level.as_str()), Some(*level));
            assert_eq!(level.index(), i);
        }
        assert_eq!(Level::parse("xs"), None);
        assert_eq!(Level::parse(""), None);
        assert_eq!(LEVELS.map(Level::as_str), ["XS", "S", "M", "L", "XL"]);
    }

    #[test]
    fn an_unknown_override_falls_through_to_the_assessment() {
        let t = ticket(|t| {
            t.override_level = Some("XXL".into());
            t.assessment = Some(assessment_of(criteria_with_sum(4), ""));
        });
        assert_eq!(effective_level(&t), Some(Level::M));
    }

    #[test]
    fn with_included_assignees_leaves_an_untouched_ticket_equal() {
        let t = ticket_of("a");
        assert_eq!(with_included_assignees(&t, &excluding_two()), t);
        let shared = ticket(|t| t.assignees = vec![person("1"), person("2")]);
        let seen = with_included_assignees(&shared, &excluding_two());
        assert_eq!(seen.assignees, vec![person("1")]);
        assert_eq!(seen.id, shared.id);
    }

    #[test]
    fn the_default_points_and_scope_serialize_like_the_ts_objects() {
        assert_eq!(
            serde_json::to_string(&DEFAULT_TICKET_POINTS).unwrap(),
            r#"{"XS":3,"S":8,"M":16,"L":32,"XL":48}"#
        );
        assert_eq!(DEFAULT_TICKET_POINTS[Level::M], 16.0);
        let s = scope(|s| {
            s.space_ids = Some(strings(&["s1"]));
            s.excluded_members = vec![ExcludedMember {
                id: "u".into(),
                name: "U".into(),
            }];
        });
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"workspaceId":"9001","name":"Acme","spaceIds":["s1"],"excludedStatuses":[],"deliveredStatuses":[],"excludedMembers":[{"id":"u","name":"U"}]}"#
        );
        let link = s.link();
        assert_eq!(link.workspace_id, "9001");
        assert_eq!(link.space_ids, Some(strings(&["s1"])));
    }
}
