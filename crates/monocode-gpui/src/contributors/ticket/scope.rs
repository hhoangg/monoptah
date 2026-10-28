//! The rules of the Scope panel (`ScopePanel` of `TicketContributorsView.tsx`)
//! as pure functions of the scope: what each checkbox shows and the scope its
//! change produces.
use monocode_core::contributors::tickets::{
    board::StatusInfo,
    tickets::{ExcludedMember, WorkspaceScope},
};

/// Statuses compare without case and surrounding space.
pub(super) fn norm(status: &str) -> String {
    status.trim().to_lowercase()
}

fn has(list: &[String], status: &str) -> bool {
    list.iter().any(|s| norm(s) == norm(status))
}

fn without(list: &[String], status: &str) -> Vec<String> {
    list.iter()
        .filter(|s| norm(s) != norm(status))
        .cloned()
        .collect()
}

pub(super) fn is_finished_type(status_type: &str) -> bool {
    status_type == "done" || status_type == "closed"
}

/// The statuses to list: the loaded ones, then any chosen name no loaded
/// ticket carries now, so a saved choice never disappears from the list.
pub(super) fn listed_statuses(statuses: &[StatusInfo], chosen: &[String]) -> Vec<StatusInfo> {
    let mut listed: Vec<StatusInfo> = Vec::new();
    for status in statuses {
        if !listed.iter().any(|s| norm(&s.name) == norm(&status.name)) {
            listed.push(status.clone());
        }
    }
    let mut chosen: Vec<&String> = chosen.iter().collect();
    chosen.sort_by(|a, b| monocode_core::contributors::model::locale_cmp(a, b));
    for name in chosen {
        if !listed.iter().any(|s| norm(&s.name) == norm(name)) {
            listed.push(StatusInfo {
                name: name.clone(),
                status_type: String::new(),
            });
        }
    }
    listed
}

/// Never an empty list: no space picked means every space.
pub(super) fn toggle_space(scope: &WorkspaceScope, id: &str, on: bool) -> WorkspaceScope {
    let current = scope.space_ids.clone().unwrap_or_default();
    let next: Vec<String> = if on {
        current
            .into_iter()
            .filter(|s| s != id)
            .chain(std::iter::once(id.to_string()))
            .collect()
    } else {
        current.into_iter().filter(|s| s != id).collect()
    };
    WorkspaceScope {
        space_ids: (!next.is_empty()).then_some(next),
        ..scope.clone()
    }
}

pub(super) fn all_spaces(scope: &WorkspaceScope) -> WorkspaceScope {
    WorkspaceScope {
        space_ids: None,
        ..scope.clone()
    }
}

pub(super) fn toggle_excluded(scope: &WorkspaceScope, status: &str, on: bool) -> WorkspaceScope {
    let mut rest = without(&scope.excluded_statuses, status);
    if on {
        rest.push(status.to_string());
    }
    WorkspaceScope {
        excluded_statuses: rest,
        ..scope.clone()
    }
}

/// Done and closed statuses are delivered unless excluded; any other status
/// is delivered only when chosen. Excluded always wins, so a status that is
/// excluded shows unchecked, and checking it lifts the exclusion.
pub(super) fn delivered_checked(scope: &WorkspaceScope, status: &StatusInfo) -> bool {
    !has(&scope.excluded_statuses, &status.name)
        && (is_finished_type(&status.status_type) || has(&scope.delivered_statuses, &status.name))
}

pub(super) fn toggle_delivered(
    scope: &WorkspaceScope,
    status: &StatusInfo,
    on: bool,
) -> WorkspaceScope {
    if is_finished_type(&status.status_type) {
        // Part of the default: leaving it out means excluding it.
        return toggle_excluded(scope, &status.name, !on);
    }
    let mut chosen = without(&scope.delivered_statuses, &status.name);
    if on {
        chosen.push(status.name.clone());
    }
    WorkspaceScope {
        delivered_statuses: chosen,
        excluded_statuses: if on {
            without(&scope.excluded_statuses, &status.name)
        } else {
            scope.excluded_statuses.clone()
        },
        ..scope.clone()
    }
}

pub(super) fn restore_member(scope: &WorkspaceScope, id: &str) -> WorkspaceScope {
    WorkspaceScope {
        excluded_members: scope
            .excluded_members
            .iter()
            .filter(|m| m.id != id)
            .cloned()
            .collect(),
        ..scope.clone()
    }
}

/// The scope with a member left off the board; unchanged when already left off.
pub(super) fn exclude_member(scope: &WorkspaceScope, id: &str, name: &str) -> WorkspaceScope {
    let mut next = scope.clone();
    if !next.excluded_members.iter().any(|m| m.id == id) {
        next.excluded_members.push(ExcludedMember {
            id: id.into(),
            name: name.into(),
        });
    }
    next
}

/// Every status of the loaded tickets is excluded (and there is at least one).
pub(super) fn all_excluded(statuses: &[StatusInfo], scope: Option<&WorkspaceScope>) -> bool {
    let excluded: &[String] = scope.map_or(&[], |s| &s.excluded_statuses);
    !statuses.is_empty() && statuses.iter().all(|s| has(excluded, &s.name))
}

pub(super) fn is_excluded(scope: &WorkspaceScope, status: &str) -> bool {
    has(&scope.excluded_statuses, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> WorkspaceScope {
        WorkspaceScope {
            workspace_id: "w1".into(),
            name: "Acme".into(),
            space_ids: None,
            excluded_statuses: vec![],
            delivered_statuses: vec![],
            excluded_members: vec![],
        }
    }

    fn status(name: &str, status_type: &str) -> StatusInfo {
        StatusInfo {
            name: name.into(),
            status_type: status_type.into(),
        }
    }

    fn statuses() -> Vec<StatusInfo> {
        vec![
            status("closed", "closed"),
            status("complete", "done"),
            status("ready for testing", "custom"),
        ]
    }

    #[test]
    fn a_space_pick_is_added_and_the_last_one_removed_means_every_space() {
        let picked = toggle_space(&scope(), "s1", true);
        assert_eq!(picked.space_ids, Some(vec!["s1".to_string()]));
        let two = toggle_space(&picked, "s2", true);
        assert_eq!(
            two.space_ids,
            Some(vec!["s1".to_string(), "s2".to_string()])
        );
        // Picked again: it moves to the end, once.
        let again = toggle_space(&two, "s1", true);
        assert_eq!(
            again.space_ids,
            Some(vec!["s2".to_string(), "s1".to_string()])
        );
        let one = toggle_space(&two, "s1", false);
        assert_eq!(one.space_ids, Some(vec!["s2".to_string()]));
        // Never an empty list.
        assert_eq!(toggle_space(&one, "s2", false).space_ids, None);
        assert_eq!(all_spaces(&two).space_ids, None);
    }

    #[test]
    fn a_status_is_excluded_by_the_checkbox_without_case_or_space() {
        let excluded = toggle_excluded(&scope(), "complete", true);
        assert_eq!(excluded.excluded_statuses, ["complete"]);
        assert!(is_excluded(&excluded, " Complete "));
        assert!(
            toggle_excluded(&excluded, "COMPLETE", false)
                .excluded_statuses
                .is_empty()
        );
        // Excluded twice is listed once.
        assert_eq!(
            toggle_excluded(&excluded, "Complete", true).excluded_statuses,
            ["Complete"]
        );
    }

    #[test]
    fn done_and_closed_statuses_are_delivered_by_default_and_custom_ones_are_not() {
        let scope = scope();
        let list = statuses();
        assert!(delivered_checked(&scope, &list[0]));
        assert!(delivered_checked(&scope, &list[1]));
        assert!(!delivered_checked(&scope, &list[2]));
    }

    #[test]
    fn a_custom_status_joins_and_leaves_the_delivered_list() {
        let custom = status("ready for testing", "custom");
        let added = toggle_delivered(&scope(), &custom, true);
        assert_eq!(
            added,
            WorkspaceScope {
                delivered_statuses: vec!["ready for testing".into()],
                ..scope()
            }
        );
        assert!(delivered_checked(&added, &custom));
        let two = WorkspaceScope {
            delivered_statuses: vec!["ready for testing".into(), "other".into()],
            ..scope()
        };
        assert_eq!(
            toggle_delivered(&two, &custom, false),
            WorkspaceScope {
                delivered_statuses: vec!["other".into()],
                ..scope()
            }
        );
    }

    #[test]
    fn unchecking_a_done_status_excludes_it_and_rechecking_lifts_the_exclusion() {
        let done = status("complete", "done");
        let off = toggle_delivered(&scope(), &done, false);
        assert_eq!(off.excluded_statuses, ["complete"]);
        assert!(!delivered_checked(&off, &done));
        let with_other = WorkspaceScope {
            excluded_statuses: vec!["complete".into(), "x".into()],
            ..scope()
        };
        assert!(!delivered_checked(&with_other, &done));
        assert_eq!(
            toggle_delivered(&with_other, &done, true),
            WorkspaceScope {
                excluded_statuses: vec!["x".into()],
                ..scope()
            }
        );
    }

    #[test]
    fn checking_an_excluded_custom_status_delivers_it_and_lifts_the_exclusion() {
        let custom = status("ready for testing", "custom");
        let both = WorkspaceScope {
            delivered_statuses: vec!["ready for testing".into()],
            excluded_statuses: vec!["ready for testing".into()],
            ..scope()
        };
        assert!(!delivered_checked(&both, &custom), "excluded always wins");
        assert_eq!(
            toggle_delivered(&both, &custom, true),
            WorkspaceScope {
                delivered_statuses: vec!["ready for testing".into()],
                excluded_statuses: vec![],
                ..scope()
            }
        );
    }

    #[test]
    fn a_chosen_status_stays_listed_even_when_no_loaded_ticket_has_it() {
        let scope = WorkspaceScope {
            delivered_statuses: vec!["gone status".into(), "Closed".into()],
            ..scope()
        };
        let listed = listed_statuses(&statuses(), &scope.delivered_statuses);
        let names: Vec<&str> = listed.iter().map(|s| s.name.as_str()).collect();
        // "Closed" is the loaded "closed"; the one nobody has is added with no type.
        assert_eq!(
            names,
            ["closed", "complete", "ready for testing", "gone status"]
        );
        assert_eq!(listed[3].status_type, "");
        assert!(delivered_checked(&scope, &listed[3]));
    }

    #[test]
    fn chosen_statuses_nobody_has_are_listed_in_name_order() {
        let listed = listed_statuses(&[], &["b".into(), "a".into(), "B".into()]);
        let names: Vec<&str> = listed.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn members_leave_and_come_back_to_the_board() {
        let left = exclude_member(&scope(), "b", "Beta");
        assert_eq!(left.excluded_members.len(), 1);
        // Excluded again: unchanged.
        assert_eq!(exclude_member(&left, "b", "Beta Renamed"), left);
        let two = exclude_member(&left, "z", "Zed");
        assert_eq!(
            two.excluded_members
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "z"]
        );
        let back = restore_member(&two, "b");
        assert_eq!(back.excluded_members.len(), 1);
        assert_eq!(back.excluded_members[0].name, "Zed");
    }

    #[test]
    fn only_a_full_exclusion_of_the_loaded_statuses_warns() {
        let list = statuses();
        let all = WorkspaceScope {
            excluded_statuses: vec![
                "closed".into(),
                "complete".into(),
                "Ready For Testing".into(),
            ],
            ..scope()
        };
        assert!(all_excluded(&list, Some(&all)));
        let some = WorkspaceScope {
            excluded_statuses: vec!["closed".into()],
            ..scope()
        };
        assert!(!all_excluded(&list, Some(&some)));
        assert!(!all_excluded(&[], Some(&all)), "no statuses, no warning");
        assert!(!all_excluded(&list, None));
    }
}
