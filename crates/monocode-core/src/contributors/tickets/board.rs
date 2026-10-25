//! The member board of a period: which tickets count, the points split and
//! the pick of tickets to assess. Port of `ticket-contributors/model/board.ts`.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

use super::tickets::{
    delivered_at, effective_level, is_active, is_delivered, with_included_assignees, ByLevel,
    ExcludedMember, Level, LevelCounts, Points, TicketRecord, WorkspaceScope,
};
use crate::contributors::model::period::{is_in_period, parse_iso, Period};
use crate::contributors::model::{js_trim, locale_cmp, OrderedMap};

/// The start of the All time window: ClickUp tickets cannot be older than this.
pub const ALL_START: &str = "2015-01-01T00:00:00Z";

#[derive(Clone, Debug, PartialEq)]
pub struct MemberTicket {
    pub ticket: TicketRecord,
    pub level: Option<Level>,
    /// What the ticket is worth in total.
    pub points: f64,
    /// This member's part: `points` split equally between the assignees.
    pub share: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemberRow {
    pub id: String,
    pub name: String,
    pub points: f64,
    pub tickets: usize,
    pub assessed: usize,
    /// Tickets in a working status in the period. Not scored.
    pub active: usize,
    pub levels: LevelCounts,
    pub ticket_list: Vec<MemberTicket>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unassigned {
    pub tickets: usize,
    pub assessed: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    pub total: usize,
    pub assessed: usize,
    pub members: Vec<MemberRow>,
    pub unassigned: Unassigned,
    /// Some counted ticket was dated by its last update, not by a delivery date.
    pub dates_approximate: bool,
}

/// Tickets delivered in the period inside the workspace scope. Excluded members
/// are taken off the tickets, so everything built from these sees only the
/// included assignees; a ticket left with none drops out.
pub fn period_tickets(
    tickets: &[TicketRecord],
    scope: &WorkspaceScope,
    period: &Period,
) -> Vec<TicketRecord> {
    tickets
        .iter()
        .filter(|t| is_delivered(t, scope) && is_in_period(period, delivered_at(t).at))
        .map(|t| with_included_assignees(t, scope))
        .collect()
}

/// Tickets in a working status that were updated in the period, with included assignees only.
pub fn active_tickets(
    tickets: &[TicketRecord],
    scope: &WorkspaceScope,
    period: &Period,
) -> Vec<TicketRecord> {
    tickets
        .iter()
        .filter(|t| is_active(t, scope) && is_in_period(period, &t.updated_at))
        .map(|t| with_included_assignees(t, scope))
        .collect()
}

/// Active tickets per member id, in the order members first appear; a ticket
/// counts once for each distinct assignee.
pub fn active_by_member(
    tickets: &[TicketRecord],
    scope: &WorkspaceScope,
    period: &Period,
) -> Vec<(String, usize)> {
    let mut counts: OrderedMap<usize> = OrderedMap::new();
    for t in active_tickets(tickets, scope, period) {
        for (id, _) in distinct_assignees(&t) {
            *counts.entry(id, || 0) += 1;
        }
    }
    counts.into_pairs().collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusInfo {
    pub name: String,
    pub status_type: String,
}

/// Distinct statuses of the tickets (first spelling wins), sorted by type then name.
pub fn statuses_of(tickets: &[TicketRecord]) -> Vec<StatusInfo> {
    let mut seen: OrderedMap<StatusInfo> = OrderedMap::new();
    for t in tickets {
        let name = js_trim(&t.status);
        let key = name.to_lowercase();
        if !key.is_empty() {
            seen.entry(&key, || StatusInfo {
                name: name.to_string(),
                status_type: t.status_type.clone(),
            });
        }
    }
    let mut statuses: Vec<StatusInfo> = seen.into_values().collect();
    statuses.sort_by(|a, b| {
        locale_cmp(&a.status_type, &b.status_type).then_with(|| locale_cmp(&a.name, &b.name))
    });
    statuses
}

/// The excluded members with their current names: the name a loaded ticket
/// carries, else the one stored when they were excluded. Borrows the scope's
/// own list when no name changed, so the caller can tell whether to save.
pub fn excluded_members_of<'a>(
    tickets: &[TicketRecord],
    scope: &'a WorkspaceScope,
) -> Cow<'a, [ExcludedMember]> {
    if scope.excluded_members.is_empty() {
        return Cow::Borrowed(&scope.excluded_members);
    }
    let mut names: HashMap<&str, &str> = HashMap::new();
    for t in tickets {
        for a in &t.assignees {
            let name = js_trim(&a.name);
            if !name.is_empty() {
                names.entry(a.id.as_str()).or_insert(name);
            }
        }
    }
    let mut changed = false;
    let next: Vec<ExcludedMember> = scope
        .excluded_members
        .iter()
        .map(|m| match names.get(m.id.as_str()) {
            Some(name) if *name != m.name => {
                changed = true;
                ExcludedMember {
                    id: m.id.clone(),
                    name: (*name).to_string(),
                }
            }
            _ => m.clone(),
        })
        .collect();
    if changed {
        Cow::Owned(next)
    } else {
        Cow::Borrowed(&scope.excluded_members)
    }
}

/// Distinct assignees by id as (id, name), first occurrence wins.
fn distinct_assignees(t: &TicketRecord) -> Vec<(&str, &str)> {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for a in &t.assignees {
        if !seen.iter().any(|(id, _)| *id == a.id) {
            seen.push((&a.id, &a.name));
        }
    }
    seen
}

/// The board of already period-filtered tickets. A ticket's points are split
/// equally between its distinct assignees; an unassessed ticket is worth 0.
/// `active` (see `active_tickets`) only adds each member's Active count, and
/// lists members who have nothing else. Members are sorted by points, highest
/// first, then by name.
pub fn build_board(tickets: &[TicketRecord], points: &Points, active: &[TicketRecord]) -> Board {
    let mut rows: OrderedMap<MemberRow> = OrderedMap::new();
    let mut unassigned = Unassigned {
        tickets: 0,
        assessed: 0,
    };
    let mut assessed = 0;
    let mut dates_approximate = false;

    fn row_of<'a>(rows: &'a mut OrderedMap<MemberRow>, id: &str, name: &str) -> &'a mut MemberRow {
        rows.entry(id, || MemberRow {
            id: id.to_string(),
            name: name.to_string(),
            points: 0.0,
            tickets: 0,
            assessed: 0,
            active: 0,
            levels: ByLevel([0; 5]),
            ticket_list: vec![],
        })
    }

    for ticket in tickets {
        if delivered_at(ticket).approximate {
            dates_approximate = true;
        }
        let level = effective_level(ticket);
        let worth = level.map_or(0.0, |l| points[l]);
        if level.is_some() {
            assessed += 1;
        }

        let members = distinct_assignees(ticket);
        if members.is_empty() {
            unassigned.tickets += 1;
            if level.is_some() {
                unassigned.assessed += 1;
            }
            continue;
        }
        let share = worth / members.len() as f64;
        for (id, name) in members {
            let row = row_of(&mut rows, id, name);
            row.points += share;
            row.tickets += 1;
            if let Some(level) = level {
                row.assessed += 1;
                row.levels[level] += 1;
            }
            row.ticket_list.push(MemberTicket {
                ticket: ticket.clone(),
                level,
                points: worth,
                share,
            });
        }
    }

    for ticket in active {
        for (id, name) in distinct_assignees(ticket) {
            row_of(&mut rows, id, name).active += 1;
        }
    }

    let mut members: Vec<MemberRow> = rows.into_values().collect();
    members.sort_by(|a, b| {
        // `b.points - a.points || ...`: a zero or NaN difference falls through
        b.points
            .partial_cmp(&a.points)
            .filter(|o| *o != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
            .then_with(|| locale_cmp(&a.name, &b.name))
            .then_with(|| locale_cmp(&a.id, &b.id))
    });
    Board {
        total: tickets.len(),
        assessed,
        members,
        unassigned,
        dates_approximate,
    }
}

/// How many more tickets to assess to cover `percent` of the `total`:
/// `max(0, ceil(percent * total / 100) - assessed)`.
pub fn to_assess_count(total: usize, assessed: usize, percent: f64) -> usize {
    let p = if percent.is_finite() {
        percent.clamp(0.0, 100.0)
    } else {
        0.0
    };
    let target = (p * total as f64 / 100.0).ceil() as usize;
    target.saturating_sub(assessed)
}

fn delivered_time(t: &TicketRecord) -> f64 {
    parse_iso(delivered_at(t).at).map_or(f64::INFINITY, |d| d.timestamp_millis() as f64)
}

fn by_delivered_then_id(a: &TicketRecord, b: &TicketRecord) -> Ordering {
    let (ta, tb) = (delivered_time(a), delivered_time(b));
    if ta != tb {
        return if ta < tb {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    locale_cmp(&a.id, &b.id)
}

/// The ids to assess in one run: `count` unassessed tickets, round-robin across
/// members (ordered by their oldest unassessed ticket, then id), each member's
/// oldest-delivered ticket first. A ticket with several assignees is taken once,
/// at its first member's turn. Unassigned tickets come last, oldest first.
pub fn pick_for_run(tickets: &[TicketRecord], count: usize) -> Vec<String> {
    if count == 0 {
        return vec![];
    }
    let mut open: Vec<&TicketRecord> = tickets
        .iter()
        .filter(|t| effective_level(t).is_none())
        .collect();
    open.sort_by(|a, b| by_delivered_then_id(a, b));

    let mut queues: OrderedMap<VecDeque<&TicketRecord>> = OrderedMap::new();
    let mut unassigned: Vec<&TicketRecord> = Vec::new();
    for ticket in open {
        let members = distinct_assignees(ticket);
        if members.is_empty() {
            unassigned.push(ticket);
        }
        for (id, _) in members {
            queues.entry(id, VecDeque::new).push_back(ticket);
        }
    }

    // Queues are already oldest first, so the head is the member's oldest ticket.
    let mut order: Vec<(String, VecDeque<&TicketRecord>)> = queues.into_pairs().collect();
    order.sort_by(|(id_a, a), (id_b, b)| {
        by_delivered_then_id(a[0], b[0]).then_with(|| locale_cmp(id_a, id_b))
    });

    let mut picked: Vec<String> = Vec::new();
    let mut taken: HashSet<&str> = HashSet::new();
    let mut progressed = true;
    while picked.len() < count && progressed {
        progressed = false;
        for (_, queue) in &mut order {
            if picked.len() >= count {
                break;
            }
            while queue.front().is_some_and(|t| taken.contains(t.id.as_str())) {
                queue.pop_front();
            }
            let Some(next) = queue.pop_front() else {
                continue;
            };
            taken.insert(&next.id);
            picked.push(next.id.clone());
            progressed = true;
        }
    }
    for ticket in unassigned {
        if picked.len() >= count {
            break;
        }
        picked.push(ticket.id.clone());
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::period::{Period, PeriodKind};
    use crate::contributors::model::test_fixtures::local_date;
    use crate::contributors::tickets::test_fixtures::{
        assessment, named, strings, ticket as base_ticket, ticket_with, user,
    };
    use crate::contributors::tickets::tickets::{TicketAssignee, DEFAULT_TICKET_POINTS};
    use std::borrow::Cow;

    fn ticket(id: &str, patch: impl FnOnce(&mut TicketRecord)) -> TicketRecord {
        ticket_with(id, patch)
    }

    fn override_of(level: &str) -> Option<String> {
        Some(level.into())
    }

    fn done_at(t: &mut TicketRecord, at: &str) {
        t.done_at = Some(at.into());
    }

    fn pairs(board: &Board) -> Vec<(String, f64)> {
        board
            .members
            .iter()
            .map(|m| (m.id.clone(), m.points))
            .collect()
    }

    fn ids(tickets: &[TicketRecord]) -> Vec<&str> {
        tickets.iter().map(|t| t.id.as_str()).collect()
    }

    // buildBoard

    #[test]
    fn build_board_splits_points_equally_between_distinct_assignees() {
        let board = build_board(
            &[
                // Score 1 per criterion: sum 5, level index 2 = M = 16 points.
                ticket("a", |t| {
                    t.assignees = vec![user("1"), user("2"), user("1")];
                    t.assessment = Some(assessment(1));
                }),
                // Override XS = 3 points, one assignee.
                ticket("b", |t| {
                    t.assignees = vec![user("1")];
                    t.override_level = override_of("XS");
                }),
            ],
            &DEFAULT_TICKET_POINTS,
            &[],
        );
        assert_eq!(
            pairs(&board),
            [("1".to_string(), 11.0), ("2".to_string(), 8.0)]
        );
        let first = &board.members[0];
        let list: Vec<(&str, Option<Level>, f64, f64)> = first
            .ticket_list
            .iter()
            .map(|t| (t.ticket.id.as_str(), t.level, t.points, t.share))
            .collect();
        assert_eq!(
            list,
            [
                ("a", Some(Level::M), 16.0, 8.0),
                ("b", Some(Level::Xs), 3.0, 3.0)
            ]
        );
        assert_eq!(first.levels, ByLevel([1, 0, 1, 0, 0]));
        assert_eq!(board.total, 2);
        assert_eq!(board.assessed, 2);
    }

    #[test]
    fn build_board_a_members_assessed_count_includes_overrides() {
        let board = build_board(
            &[
                ticket("a", |t| {
                    t.assignees = vec![user("1")];
                    t.assessment = Some(assessment(0));
                }),
                ticket("b", |t| {
                    t.assignees = vec![user("1")];
                    t.override_level = override_of("L");
                }),
                ticket("c", |t| t.assignees = vec![user("1")]),
            ],
            &DEFAULT_TICKET_POINTS,
            &[],
        );
        let m = &board.members[0];
        assert_eq!(m.tickets, 3);
        assert_eq!(m.assessed, 2);
        assert_eq!(m.points, 3.0 + 32.0);
        let open = m.ticket_list.iter().find(|t| t.ticket.id == "c").unwrap();
        assert_eq!((open.level, open.points, open.share), (None, 0.0, 0.0));
        assert_eq!(board.assessed, 2);
    }

    #[test]
    fn build_board_counts_unassigned_tickets_apart() {
        let board = build_board(
            &[
                ticket("a", |t| t.override_level = override_of("S")),
                ticket("b", |_| {}),
                ticket("c", |t| {
                    t.assignees = vec![user("1")];
                    t.override_level = override_of("S");
                }),
            ],
            &DEFAULT_TICKET_POINTS,
            &[],
        );
        assert_eq!(
            board.unassigned,
            Unassigned {
                tickets: 2,
                assessed: 1
            }
        );
        assert_eq!(board.members.len(), 1);
        assert_eq!(board.total, 3);
        assert_eq!(board.assessed, 2);
    }

    #[test]
    fn build_board_sorts_members_by_points_desc_then_name() {
        let board = build_board(
            &[
                ticket("a", |t| {
                    t.assignees = vec![named("1", "Zed")];
                    t.override_level = override_of("S");
                }),
                ticket("b", |t| {
                    t.assignees = vec![named("2", "Amy")];
                    t.override_level = override_of("S");
                }),
                ticket("c", |t| {
                    t.assignees = vec![named("3", "Bob")];
                    t.override_level = override_of("XL");
                }),
            ],
            &DEFAULT_TICKET_POINTS,
            &[],
        );
        let names: Vec<&str> = board.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Bob", "Amy", "Zed"]);
    }

    #[test]
    fn build_board_uses_the_given_points_table() {
        let mut points = DEFAULT_TICKET_POINTS;
        points[Level::Xs] = 5.0;
        let board = build_board(
            &[ticket("a", |t| {
                t.assignees = vec![user("1")];
                t.override_level = override_of("XS");
            })],
            &points,
            &[],
        );
        assert_eq!(board.members[0].points, 5.0);
    }

    // toAssessCount

    #[test]
    fn to_assess_count_covers_p0_p100_a_above_target_n0() {
        assert_eq!(to_assess_count(10, 0, 0.0), 0);
        assert_eq!(to_assess_count(10, 0, 100.0), 10);
        assert_eq!(to_assess_count(10, 4, 100.0), 6);
        assert_eq!(to_assess_count(10, 4, 50.0), 1);
        assert_eq!(to_assess_count(10, 8, 50.0), 0);
        assert_eq!(to_assess_count(0, 0, 100.0), 0);
        assert_eq!(to_assess_count(0, 0, 0.0), 0);
    }

    #[test]
    fn to_assess_count_rounds_the_target_up_and_clamps_the_percent() {
        assert_eq!(to_assess_count(7, 0, 50.0), 4);
        assert_eq!(to_assess_count(10, 0, 150.0), 10);
        assert_eq!(to_assess_count(10, 0, -20.0), 0);
        assert_eq!(to_assess_count(10, 0, f64::NAN), 0);
    }

    // pickForRun

    fn owned(id: &str, member: &str, at: &str) -> TicketRecord {
        ticket(id, |t| {
            t.assignees = vec![user(member)];
            done_at(t, at);
        })
    }

    #[test]
    fn pick_for_run_interleaves_members_oldest_first() {
        let tickets = [
            owned("a1", "1", "2026-10-01T00:00:00Z"),
            owned("a2", "1", "2026-10-03T00:00:00Z"),
            owned("a3", "1", "2026-10-05T00:00:00Z"),
            owned("b1", "2", "2026-10-02T00:00:00Z"),
            owned("b2", "2", "2026-10-04T00:00:00Z"),
        ];
        assert_eq!(pick_for_run(&tickets, 5), ["a1", "b1", "a2", "b2", "a3"]);
        assert_eq!(pick_for_run(&tickets, 3), ["a1", "b1", "a2"]);
    }

    #[test]
    fn pick_for_run_skips_tickets_that_already_have_an_assessment_or_an_override() {
        let tickets = [
            ticket("a1", |t| {
                t.assignees = vec![user("1")];
                done_at(t, "2026-10-01T00:00:00Z");
                t.assessment = Some(assessment(1));
            }),
            ticket("a2", |t| {
                t.assignees = vec![user("1")];
                done_at(t, "2026-10-02T00:00:00Z");
                t.override_level = override_of("S");
            }),
            owned("a3", "1", "2026-10-03T00:00:00Z"),
        ];
        assert_eq!(pick_for_run(&tickets, 10), ["a3"]);
    }

    #[test]
    fn pick_for_run_takes_a_multi_assignee_ticket_once_at_its_first_members_turn() {
        let tickets = [
            ticket("shared", |t| {
                t.assignees = vec![user("1"), user("2")];
                done_at(t, "2026-10-01T00:00:00Z");
            }),
            owned("a2", "1", "2026-10-03T00:00:00Z"),
            owned("b2", "2", "2026-10-04T00:00:00Z"),
        ];
        let picked = pick_for_run(&tickets, 10);
        assert_eq!(picked, ["shared", "b2", "a2"]);
        let distinct: std::collections::HashSet<&String> = picked.iter().collect();
        assert_eq!(distinct.len(), picked.len());
    }

    #[test]
    fn pick_for_run_puts_unassigned_tickets_last_oldest_first() {
        let tickets = [
            ticket("u2", |t| done_at(t, "2026-10-02T00:00:00Z")),
            ticket("u1", |t| done_at(t, "2026-09-01T00:00:00Z")),
            owned("a1", "1", "2026-10-05T00:00:00Z"),
        ];
        assert_eq!(pick_for_run(&tickets, 10), ["a1", "u1", "u2"]);
        assert_eq!(pick_for_run(&tickets, 2), ["a1", "u1"]);
    }

    #[test]
    fn pick_for_run_returns_nothing_for_a_count_of_zero() {
        assert!(pick_for_run(&[base_ticket("a")], 0).is_empty());
    }

    // periodTickets

    fn scope_of(patch: impl FnOnce(&mut WorkspaceScope)) -> WorkspaceScope {
        let mut s = WorkspaceScope {
            workspace_id: "w1".into(),
            name: "Workspace".into(),
            space_ids: Some(strings(&["s1"])),
            excluded_statuses: strings(&["Cancelled"]),
            delivered_statuses: vec![],
            excluded_members: vec![],
        };
        patch(&mut s);
        s
    }

    fn october() -> Period {
        Period {
            kind: PeriodKind::Month,
            id: "2026-10".into(),
            label: "Oct 2026".into(),
            start: local_date(2026, 9, 1),
            end: local_date(2026, 10, 1),
        }
    }

    const IN_OCT: &str = "2026-10-10T12:00:00";

    #[test]
    fn period_tickets_drops_other_workspaces_spaces_excluded_statuses_other_periods() {
        let kept = ticket("kept", |t| done_at(t, IN_OCT));
        let result = period_tickets(
            &[
                kept.clone(),
                ticket("other-ws", |t| {
                    t.workspace_id = "w2".into();
                    done_at(t, IN_OCT);
                }),
                ticket("other-space", |t| {
                    t.space_id = "s2".into();
                    done_at(t, IN_OCT);
                }),
                ticket("cancelled", |t| {
                    t.status = "cancelled".into();
                    done_at(t, IN_OCT);
                }),
                ticket("september", |t| done_at(t, "2026-09-30T12:00:00")),
                ticket("november", |t| done_at(t, "2026-11-01T12:00:00")),
            ],
            &scope_of(|_| {}),
            &october(),
        );
        assert_eq!(result, [kept]);
    }

    #[test]
    fn period_tickets_keeps_a_ticket_done_in_the_period_even_if_edited_after_it() {
        let t = ticket("late-edit", |t| {
            done_at(t, IN_OCT);
            t.updated_at = "2026-12-24T09:00:00".into();
        });
        assert_eq!(
            period_tickets(std::slice::from_ref(&t), &scope_of(|_| {}), &october()),
            [t]
        );
    }

    #[test]
    fn period_tickets_drops_a_ticket_edited_in_the_period_but_done_before_it() {
        let t = ticket("early", |t| {
            done_at(t, "2026-08-01T00:00:00");
            t.updated_at = IN_OCT.into();
        });
        assert!(period_tickets(&[t], &scope_of(|_| {}), &october()).is_empty());
    }

    #[test]
    fn period_tickets_counts_a_custom_delivered_status_by_status_since_else_updated_at() {
        let chosen = scope_of(|s| s.delivered_statuses = strings(&["ready for testing"]));
        let custom = |id: &str, patch: &dyn Fn(&mut TicketRecord)| {
            ticket(id, |t| {
                t.status = "Ready For Testing".into();
                t.status_type = "custom".into();
                t.done_at = None;
                patch(t);
            })
        };
        let exact = custom("exact", &|t| {
            t.status_since = Some(IN_OCT.into());
            t.updated_at = "2026-12-01T00:00:00".into();
        });
        let approx = custom("approx", &|t| t.updated_at = IN_OCT.into());
        let not_chosen = custom("working", &|t| {
            t.status = "in progress".into();
            t.updated_at = IN_OCT.into();
        });
        let shown = period_tickets(&[exact, approx, not_chosen], &chosen, &october());
        assert_eq!(ids(&shown), ["exact", "approx"]);
    }

    #[test]
    fn period_tickets_uses_done_and_closed_types_by_default_and_ignores_custom_ones() {
        let custom = ticket("c", |t| {
            t.status_type = "custom".into();
            t.done_at = None;
        });
        let done = ticket("d", |t| {
            t.status_type = "done".into();
            done_at(t, IN_OCT);
        });
        assert_eq!(
            period_tickets(&[custom, done.clone()], &scope_of(|_| {}), &october()),
            [done]
        );
    }

    #[test]
    fn period_tickets_excluded_wins_over_a_delivered_status() {
        let s = scope_of(|s| {
            s.delivered_statuses = strings(&["cancelled"]);
            s.excluded_statuses = strings(&["Cancelled"]);
        });
        let t = ticket("x", |t| {
            t.status = "cancelled".into();
            done_at(t, IN_OCT);
        });
        assert!(period_tickets(&[t], &s, &october()).is_empty());
    }

    #[test]
    fn period_tickets_drops_a_parent_with_subtasks_and_keeps_its_subtask() {
        let parent = ticket("parent", |t| {
            done_at(t, IN_OCT);
            t.has_subtasks = true;
        });
        let sub = ticket("sub", |t| {
            done_at(t, IN_OCT);
            t.parent_id = Some("parent".into());
        });
        assert_eq!(
            period_tickets(&[parent, sub.clone()], &scope_of(|_| {}), &october()),
            [sub]
        );
    }

    #[test]
    fn period_tickets_never_lists_active_work() {
        let active = ticket("a", |t| {
            t.status = "in progress".into();
            t.status_type = "custom".into();
            t.done_at = None;
            t.updated_at = IN_OCT.into();
        });
        assert!(period_tickets(&[active], &scope_of(|_| {}), &october()).is_empty());
    }

    // active work

    fn active_scope() -> WorkspaceScope {
        scope_of(|s| {
            s.space_ids = None;
            s.excluded_statuses = vec![];
        })
    }

    fn working(id: &str, patch: impl FnOnce(&mut TicketRecord)) -> TicketRecord {
        ticket(id, |t| {
            t.status = "in progress".into();
            t.status_type = "custom".into();
            t.done_at = None;
            t.updated_at = "2026-10-10T12:00:00".into();
            t.assignees = vec![user("1")];
            patch(t);
        })
    }

    #[test]
    fn active_work_counts_active_tickets_per_assignee_by_updated_at_in_the_period() {
        let counts = active_by_member(
            &[
                working("a", |_| {}),
                working("b", |t| t.assignees = vec![user("1"), user("2")]),
                working("old", |t| t.updated_at = "2026-09-10T12:00:00".into()),
                working("done", |t| {
                    t.status_type = "done".into();
                    done_at(t, "2026-10-10T12:00:00");
                }),
                working("open", |t| t.status_type = "open".into()),
            ],
            &active_scope(),
            &october(),
        );
        assert_eq!(counts, [("1".to_string(), 2), ("2".to_string(), 1)]);
    }

    #[test]
    fn active_work_an_active_ticket_is_in_active_tickets_and_not_in_period_tickets() {
        let t = working("a", |_| {});
        let shown = active_tickets(std::slice::from_ref(&t), &active_scope(), &october());
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0], t);
        assert!(period_tickets(&[t], &active_scope(), &october()).is_empty());
    }

    #[test]
    fn active_work_build_board_adds_the_active_count_and_lists_active_only_members() {
        let board = build_board(
            &[ticket("d", |t| {
                t.assignees = vec![named("1", "Ann")];
                t.override_level = override_of("XS");
            })],
            &DEFAULT_TICKET_POINTS,
            &[
                working("a", |t| t.assignees = vec![named("1", "Ann")]),
                working("b", |t| t.assignees = vec![named("2", "Bob")]),
            ],
        );
        let rows: Vec<(&str, f64, usize, usize)> = board
            .members
            .iter()
            .map(|m| (m.id.as_str(), m.points, m.tickets, m.active))
            .collect();
        assert_eq!(rows, [("1", 3.0, 1, 1), ("2", 0.0, 0, 1)]);
        assert_eq!(board.members[1].name, "Bob");
        assert!(board.members[1].ticket_list.is_empty());
        assert_eq!(board.total, 1);
    }

    #[test]
    fn active_work_active_is_0_when_there_is_no_active_work() {
        let board = build_board(
            &[ticket("d", |t| t.assignees = vec![user("1")])],
            &DEFAULT_TICKET_POINTS,
            &[],
        );
        assert_eq!(board.members[0].active, 0);
    }

    // datesApproximate

    #[test]
    fn dates_approximate_is_set_when_a_counted_ticket_fell_back_to_updated_at() {
        let exact = ticket("e", |t| t.assignees = vec![user("1")]);
        let approx = ticket("a", |t| {
            t.assignees = vec![user("1")];
            t.status_type = "custom".into();
            t.done_at = None;
        });
        let approximate = |tickets: &[TicketRecord]| {
            build_board(tickets, &DEFAULT_TICKET_POINTS, &[]).dates_approximate
        };
        assert!(!approximate(std::slice::from_ref(&exact)));
        assert!(approximate(&[exact, approx]));
        assert!(!approximate(&[]));
    }

    #[test]
    fn dates_approximate_is_not_set_by_status_since() {
        let t = ticket("s", |t| {
            t.status_type = "custom".into();
            t.done_at = None;
            t.status_since = Some("2026-10-05T00:00:00Z".into());
        });
        assert!(!build_board(&[t], &DEFAULT_TICKET_POINTS, &[]).dates_approximate);
    }

    // statusesOf

    #[test]
    fn statuses_of_lists_distinct_statuses_sorted_by_type_then_name() {
        let with = |id: &str, status: &str, kind: &str| {
            ticket(id, |t| {
                t.status = status.into();
                t.status_type = kind.into();
            })
        };
        let tickets = [
            with("1", "Done", "done"),
            with("2", "done ", "done"),
            with("3", "ready for testing", "custom"),
            with("4", "Closed", "closed"),
            with("5", "backlog", "open"),
            with("6", "in progress", "custom"),
        ];
        let listed: Vec<(String, String)> = statuses_of(&tickets)
            .into_iter()
            .map(|s| (s.name, s.status_type))
            .collect();
        let want = [
            ("Closed", "closed"),
            ("in progress", "custom"),
            ("ready for testing", "custom"),
            ("Done", "done"),
            ("backlog", "open"),
        ]
        .map(|(name, kind)| (name.to_string(), kind.to_string()));
        assert_eq!(listed, want);
    }

    #[test]
    fn statuses_of_skips_blank_statuses() {
        assert!(statuses_of(&[ticket("1", |t| t.status = "  ".into())]).is_empty());
    }

    #[test]
    fn all_start_is_the_fixed_start_of_the_all_time_window() {
        assert_eq!(ALL_START, "2015-01-01T00:00:00Z");
    }

    // excluded members

    fn excluding(members: &[(&str, &str)]) -> WorkspaceScope {
        scope_of(|s| {
            s.space_ids = None;
            s.excluded_statuses = vec![];
            s.excluded_members = members
                .iter()
                .map(|(id, name)| ExcludedMember {
                    id: (*id).into(),
                    name: (*name).into(),
                })
                .collect();
        })
    }

    fn delivered(id: &str, patch: impl FnOnce(&mut TicketRecord)) -> TicketRecord {
        ticket(id, |t| {
            done_at(t, IN_OCT);
            patch(t);
        })
    }

    #[test]
    fn excluded_members_split_a_shared_ticket_between_the_included_assignees_only() {
        let scope = excluding(&[("2", "Two")]);
        let shared = delivered("shared", |t| {
            t.assignees = vec![user("1"), user("2")];
            t.override_level = override_of("S");
        });
        let board = build_board(
            &period_tickets(std::slice::from_ref(&shared), &scope, &october()),
            &DEFAULT_TICKET_POINTS,
            &[],
        );
        assert_eq!(pairs(&board), [("1".to_string(), 8.0)]);
        let entry = &board.members[0].ticket_list[0];
        assert_eq!((entry.points, entry.share), (8.0, 8.0));
        // Without the exclusion the same ticket is split in two.
        let plain = build_board(
            &period_tickets(&[shared], &excluding(&[]), &october()),
            &DEFAULT_TICKET_POINTS,
            &[],
        );
        assert_eq!(
            pairs(&plain),
            [("1".to_string(), 4.0), ("2".to_string(), 4.0)]
        );
    }

    #[test]
    fn excluded_members_drop_a_ticket_whose_assignees_are_all_excluded_from_n_coverage_and_the_run()
    {
        let scope = excluding(&[("2", "Two")]);
        let mine = delivered("mine", |t| {
            t.assignees = vec![user("1")];
            t.override_level = override_of("XS");
        });
        let theirs = delivered("theirs", |t| t.assignees = vec![user("2")]);
        let also_theirs = delivered("also", |t| t.assignees = vec![user("2"), named("2", "Two")]);
        let open = delivered("open", |t| t.assignees = vec![user("1")]);
        let shown = period_tickets(&[mine, theirs, also_theirs, open], &scope, &october());
        assert_eq!(ids(&shown), ["mine", "open"]);

        let board = build_board(&shown, &DEFAULT_TICKET_POINTS, &[]);
        assert_eq!(board.total, 2);
        assert_eq!(board.assessed, 1);
        let members: Vec<&str> = board.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(members, ["1"]);
        assert_eq!(to_assess_count(board.total, board.assessed, 100.0), 1);
        assert_eq!(pick_for_run(&shown, 5), ["open"]);
    }

    #[test]
    fn excluded_members_keep_a_ticket_without_assignees_as_unassigned() {
        let scope = excluding(&[("2", "Two")]);
        let shown = period_tickets(
            &[
                delivered("nobody", |_| {}),
                delivered("theirs", |t| t.assignees = vec![user("2")]),
            ],
            &scope,
            &october(),
        );
        let board = build_board(&shown, &DEFAULT_TICKET_POINTS, &[]);
        assert_eq!(ids(&shown), ["nobody"]);
        assert_eq!(board.total, 1);
        assert_eq!(
            board.unassigned,
            Unassigned {
                tickets: 1,
                assessed: 0
            }
        );
        assert_eq!(pick_for_run(&shown, 1), ["nobody"]);
    }

    #[test]
    fn excluded_members_leave_tickets_alone_when_nobody_is_excluded_from_them() {
        let t = delivered("a", |t| t.assignees = vec![user("1")]);
        let shown = period_tickets(
            std::slice::from_ref(&t),
            &excluding(&[("2", "Two")]),
            &october(),
        );
        assert_eq!(shown[0], t);
    }

    #[test]
    fn excluded_members_do_not_count_excluded_members_or_their_tickets_as_active() {
        let scope = excluding(&[("2", "Two")]);
        let busy = |id: &str, assignees: Vec<TicketAssignee>| {
            ticket(id, |t| {
                t.status = "in progress".into();
                t.status_type = "custom".into();
                t.done_at = None;
                t.updated_at = IN_OCT.into();
                t.assignees = assignees;
            })
        };
        let tickets = [
            busy("a", vec![user("1")]),
            busy("b", vec![user("1"), user("2")]),
            busy("c", vec![user("2")]),
        ];
        let active = active_tickets(&tickets, &scope, &october());
        assert_eq!(ids(&active), ["a", "b"]);
        assert_eq!(
            active_by_member(&tickets, &scope, &october()),
            [("1".to_string(), 2)]
        );
        let board = build_board(&[], &DEFAULT_TICKET_POINTS, &active);
        let rows: Vec<(&str, usize)> = board
            .members
            .iter()
            .map(|m| (m.id.as_str(), m.active))
            .collect();
        assert_eq!(rows, [("1", 2)]);
    }

    #[test]
    fn excluded_members_keep_the_stored_name_unless_a_loaded_ticket_carries_a_newer_one() {
        let tickets = [delivered("a", |t| t.assignees = vec![named("2", "Bea")])];
        let stored = excluding(&[("2", "Old Bea"), ("9", "Left Lee")]);
        let members = excluded_members_of(&tickets, &stored);
        assert_eq!(
            members.as_ref(),
            [
                ExcludedMember {
                    id: "2".into(),
                    name: "Bea".into()
                },
                ExcludedMember {
                    id: "9".into(),
                    name: "Left Lee".into()
                },
            ]
        );
        // Nothing newer: the scope's own list comes back, so there is nothing to save.
        let same = excluded_members_of(&[], &stored);
        assert!(matches!(same, Cow::Borrowed(_)));
        assert_eq!(same.as_ref(), stored.excluded_members.as_slice());
    }

    // not in the TS suite

    #[test]
    fn to_assess_count_never_goes_below_zero_for_huge_assessed_counts() {
        assert_eq!(to_assess_count(3, 99, 100.0), 0);
    }

    #[test]
    fn a_ticket_with_an_unparseable_date_is_picked_last_among_the_open_ones() {
        let tickets = [
            ticket("bad", |t| {
                t.assignees = vec![user("1")];
                done_at(t, "not a date");
            }),
            owned("good", "1", "2026-10-01T00:00:00Z"),
        ];
        assert_eq!(pick_for_run(&tickets, 5), ["good", "bad"]);
    }
}
