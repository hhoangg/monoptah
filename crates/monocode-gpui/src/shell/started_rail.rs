//! startedRail.ts: open state of the rails of started sessions in the inset
//! sidebar.
//!
//! A thread's rail opens on its own while the thread or one of its sessions is
//! in play (in view, chosen or working), and then stays open until the user
//! collapses it, so several rails can be open at once. `open` holds rails that
//! stay open on their own; `closed` holds user collapses, which only suppress
//! auto-opening while the thread is still in play.
use std::{collections::HashMap, collections::HashSet, hash::Hash};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StartedRailState<T: Copy + Eq + Hash> {
    pub(super) open: HashSet<T>,
    pub(super) closed: HashSet<T>,
}
impl<T: Copy + Eq + Hash> Default for StartedRailState<T> {
    fn default() -> Self {
        Self {
            open: HashSet::new(),
            closed: HashSet::new(),
        }
    }
}

pub(super) fn started_rail_open<T: Copy + Eq + Hash>(
    state: &StartedRailState<T>,
    id: T,
    auto: bool,
) -> bool {
    state.open.contains(&id) || (auto && !state.closed.contains(&id))
}

pub(super) struct StartedRailSync<T> {
    /// Parents whose rail shows open for now: latch plus working ones.
    pub(super) auto: HashSet<T>,
    /// Parents whose own row or a child is in view or chosen: kept open once
    /// the user moves away. A working session alone does not latch.
    pub(super) latch: HashSet<T>,
    /// Every parent with a rail.
    pub(super) parents: Vec<T>,
    /// Parents whose group the focused session just moved into: reopened even
    /// if the user had collapsed them.
    pub(super) entered: HashSet<T>,
}

/// Latches in-view rails open, reopens entered ones and forgets collapses of
/// threads that are no longer in play. `None` when nothing changes.
pub(super) fn sync_started_rail<T: Copy + Eq + Hash>(
    state: &StartedRailState<T>,
    sync: &StartedRailSync<T>,
) -> Option<StartedRailState<T>> {
    let mut next = state.clone();
    for id in &sync.entered {
        next.open.insert(*id);
        next.closed.remove(id);
    }
    for id in &sync.latch {
        if !next.closed.contains(id) {
            next.open.insert(*id);
        }
    }
    for id in &sync.parents {
        if !sync.auto.contains(id) {
            next.closed.remove(id);
        }
    }
    (next != *state).then_some(next)
}

/// The sets `sync_started_rail` takes, from what the sidebar shows. `skip`
/// leaves out parents with a rail of their own kind (orchestrators).
/// `entered` needs the previous sync's focused id: only a change of it counts.
pub(super) fn started_rail_sets<T: Copy + Eq + Hash>(
    by_parent: &HashMap<T, Vec<T>>,
    skip: impl Fn(T) -> bool,
    in_view: impl Fn(T) -> bool,
    busy: impl Fn(T) -> bool,
    highlighted: Option<T>,
    previous_highlighted: Option<T>,
) -> StartedRailSync<T> {
    let parents: Vec<T> = by_parent.keys().copied().filter(|id| !skip(*id)).collect();
    let mut auto = HashSet::new();
    let mut latch = HashSet::new();
    let mut entered = HashSet::new();
    for &id in &parents {
        let children = &by_parent[&id];
        if in_view(id) || children.iter().any(|c| in_view(*c)) {
            latch.insert(id);
        }
        if latch.contains(&id) || busy(id) || children.iter().any(|c| busy(*c)) {
            auto.insert(id);
        }
        if let Some(focus) = highlighted
            && highlighted != previous_highlighted
            && (id == focus || children.contains(&focus))
        {
            entered.insert(id);
        }
    }
    StartedRailSync {
        auto,
        latch,
        parents,
        entered,
    }
}

pub(super) fn toggle_started_rail<T: Copy + Eq + Hash>(
    state: &StartedRailState<T>,
    id: T,
    auto: bool,
) -> StartedRailState<T> {
    let mut next = state.clone();
    if started_rail_open(state, id, auto) {
        next.open.remove(&id);
        next.closed.insert(id);
    } else {
        next.open.insert(id);
        next.closed.remove(&id);
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    type State = StartedRailState<&'static str>;

    fn sync(
        current: &State,
        auto: &[&'static str],
        latch: Option<&[&'static str]>,
        parents: &[&'static str],
        entered: &[&'static str],
    ) -> Option<State> {
        sync_started_rail(
            current,
            &StartedRailSync {
                auto: auto.iter().copied().collect(),
                latch: latch.unwrap_or(auto).iter().copied().collect(),
                parents: parents.to_vec(),
                entered: entered.iter().copied().collect(),
            },
        )
    }
    /// The state after a sync, or the same one when nothing changed.
    fn synced(
        current: &State,
        auto: &[&'static str],
        latch: Option<&[&'static str]>,
        parents: &[&'static str],
        entered: &[&'static str],
    ) -> State {
        sync(current, auto, latch, parents, entered).unwrap_or_else(|| current.clone())
    }
    fn state(open: &[&'static str], closed: &[&'static str]) -> State {
        StartedRailState {
            open: open.iter().copied().collect(),
            closed: closed.iter().copied().collect(),
        }
    }
    fn empty() -> State {
        StartedRailState::default()
    }

    #[test]
    fn is_closed_by_default_and_open_while_the_thread_is_in_play() {
        assert!(!started_rail_open(&empty(), "a", false));
        assert!(started_rail_open(&empty(), "a", true));
    }

    #[test]
    fn stays_closed_while_in_play_once_the_user_collapsed_it() {
        assert!(!started_rail_open(&state(&[], &["a"]), "a", true));
    }

    #[test]
    fn stays_open_when_no_longer_in_play_once_opened() {
        assert!(started_rail_open(&state(&["a"], &[]), "a", false));
    }

    #[test]
    fn keeps_an_auto_opened_rail_open_when_another_thread_becomes_active() {
        let mut current = synced(&empty(), &["a"], None, &["a", "b"], &[]);
        current = synced(&current, &["b"], None, &["a", "b"], &[]);
        assert!(started_rail_open(&current, "a", false));
        assert!(started_rail_open(&current, "b", true));
    }

    #[test]
    fn keeps_a_user_collapse_while_the_thread_is_still_in_play() {
        let mut current = toggle_started_rail(&empty(), "a", true);
        current = synced(&current, &["a"], None, &["a"], &[]);
        assert!(!started_rail_open(&current, "a", true));
        assert!(!current.open.contains("a"));
    }

    #[test]
    fn clears_the_collapse_once_the_thread_is_out_of_play_so_it_reopens() {
        let mut current = toggle_started_rail(&empty(), "a", true);
        current = synced(&current, &[], None, &["a"], &[]);
        assert!(!current.closed.contains("a"));
        assert!(!started_rail_open(&current, "a", false));
        assert!(started_rail_open(&current, "a", true));
    }

    #[test]
    fn returns_nothing_when_nothing_changes() {
        let first = synced(&empty(), &["a"], None, &["a", "b"], &[]);
        assert_eq!(sync(&first, &["a"], None, &["a", "b"], &[]), None);
        assert_eq!(sync(&empty(), &[], None, &["a"], &[]), None);
    }

    #[test]
    fn leaves_unrelated_ids_untouched() {
        let current = state(&["x"], &["y"]);
        let next = synced(&current, &["a"], None, &["a"], &[]);
        assert!(next.open.contains("x"));
        assert!(next.open.contains("a"));
        assert!(next.closed.contains("y"));
    }

    #[test]
    fn shows_a_busy_only_parent_open_but_does_not_latch_it() {
        let current = synced(&empty(), &["a"], Some(&[]), &["a", "b"], &[]);
        assert_eq!(current, empty());
        assert!(started_rail_open(&current, "a", true));
        // Busy ends: no longer auto, and it was never latched, so it collapses.
        let after = synced(&current, &[], None, &["a", "b"], &[]);
        assert!(!started_rail_open(&after, "a", false));
    }

    #[test]
    fn reopens_a_collapsed_in_play_rail_when_focus_enters_its_group() {
        let mut current = toggle_started_rail(&empty(), "a", true);
        current = synced(&current, &["a"], None, &["a"], &[]);
        assert!(!started_rail_open(&current, "a", true));
        current = synced(&current, &["a"], None, &["a"], &["a"]);
        assert!(started_rail_open(&current, "a", true));
        assert!(current.open.contains("a"));
        assert!(!current.closed.contains("a"));
    }

    #[test]
    fn does_not_reopen_a_collapsed_rail_without_an_entry() {
        let collapsed = state(&[], &["a"]);
        assert_eq!(sync(&collapsed, &["a"], None, &["a"], &[]), None);
    }

    #[test]
    fn does_not_mutate_the_previous_state() {
        let current = state(&[], &["a"]);
        let next = sync(&current, &[], None, &["a"], &[]);
        assert!(next.is_some());
        assert!(current.closed.contains("a"));
    }

    #[test]
    fn opens_a_collapsed_rail_and_collapses_it_again_on_a_second_toggle() {
        let opened = toggle_started_rail(&empty(), "a", false);
        assert!(started_rail_open(&opened, "a", false));
        let closed = toggle_started_rail(&opened, "a", false);
        assert!(!started_rail_open(&closed, "a", false));
        assert!(closed.closed.contains("a"));
    }

    #[test]
    fn collapses_an_auto_open_rail_and_reopens_it_on_the_next_toggle() {
        let closed = toggle_started_rail(&empty(), "a", true);
        assert!(!started_rail_open(&closed, "a", true));
        let reopened = toggle_started_rail(&closed, "a", true);
        assert!(started_rail_open(&reopened, "a", true));
    }

    #[test]
    fn toggling_does_not_touch_other_rails() {
        let next = toggle_started_rail(&state(&["x"], &["y"]), "a", false);
        assert!(next.open.contains("x"));
        assert!(next.closed.contains("y"));
    }

    type Parents = HashMap<&'static str, Vec<&'static str>>;
    fn by_parent() -> Parents {
        HashMap::from([("a", vec!["a1"]), ("o", vec!["o1"])])
    }
    fn sets(
        in_view: impl Fn(&'static str) -> bool,
        busy: impl Fn(&'static str) -> bool,
        highlighted: Option<&'static str>,
        previous: Option<&'static str>,
    ) -> StartedRailSync<&'static str> {
        started_rail_sets(
            &by_parent(),
            |id| id == "o",
            in_view,
            busy,
            highlighted,
            previous,
        )
    }

    #[test]
    fn leaves_skipped_parents_out_of_every_set() {
        let result = sets(|_| true, |_| false, Some("o1"), None);
        assert_eq!(result.parents, ["a"]);
        assert!(!result.latch.contains("o"));
        assert!(!result.entered.contains("o"));
    }

    #[test]
    fn latches_in_view_parents_but_only_shows_busy_ones() {
        let busy = sets(|_| false, |id| id == "a1", None, None);
        assert!(busy.auto.contains("a"));
        assert!(!busy.latch.contains("a"));
        let view = sets(|id| id == "a1", |_| false, None, None);
        assert!(view.latch.contains("a"));
    }

    #[test]
    fn enters_a_group_only_when_the_focused_id_changes() {
        let moved = sets(|_| false, |_| false, Some("a1"), Some("x"));
        assert!(moved.entered.contains("a"));
        let same = sets(|_| false, |_| false, Some("a1"), Some("a1"));
        assert!(same.entered.is_empty());
        let selection_only = sets(|id| id == "a1", |_| false, Some("x"), Some("x"));
        assert!(selection_only.entered.is_empty());
    }
}
