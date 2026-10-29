//! Archiving a session archives the sessions it started, so the group leaves the sidebar together.
use super::*;

/// Saves `ids` in order and stops at the first failure, returning the ids that
/// were saved so the sidebar can show exactly what the store holds.
pub(super) fn write_until_failure(
    ids: &[i64],
    mut write: impl FnMut(i64) -> Result<(), String>,
) -> (Vec<i64>, Option<String>) {
    let mut written = Vec::new();
    for &id in ids {
        match write(id) {
            Ok(()) => written.push(id),
            Err(error) => return (written, Some(error)),
        }
    }
    (written, None)
}

impl Shell {
    /// The sessions one archive or restore of `id` covers, in the order to save
    /// them: `id` last when archiving and first when restoring, so a write that
    /// stops early never leaves children listed under a session still archived.
    ///
    /// Archiving takes every session `id` started, directly or through a chain,
    /// that is still listed. Restoring takes back only those archived together
    /// with `id`: one the user archived on its own earlier stays archived.
    pub(super) fn archive_family(&self, id: i64, archived: bool, cx: &App) -> Vec<i64> {
        let mut family = if archived {
            let mut found: Vec<i64> = Vec::new();
            let mut frontier = vec![id];
            while let Some(parent) = frontier.pop() {
                let mut started: Vec<i64> = self
                    .started_by
                    .iter()
                    .filter(|(child, by)| **by == parent && **child != id && !found.contains(child))
                    .map(|(child, _)| *child)
                    .collect();
                started.sort_unstable();
                frontier.extend(&started);
                found.extend(started);
            }
            found
        } else {
            self.archived_with.get(&id).cloned().unwrap_or_default()
        };
        family.retain(|child| {
            !self.archiving.contains(child)
                && self.session_summaries.contains_key(child)
                && self.session_metadata(*child, cx).summary.archived != archived
        });
        if archived {
            family.push(id);
        } else {
            family.insert(0, id);
        }
        family
    }
    /// Notes which of `children` an archive of `id` took with it, for the
    /// restore that undoes it. A session toggled on its own is no longer part
    /// of what any parent archived.
    pub(super) fn remember_archived_with(&mut self, id: i64, children: &[i64], archived: bool) {
        for together in self.archived_with.values_mut() {
            together.retain(|child| *child != id);
        }
        self.archived_with
            .retain(|_, together| !together.is_empty());
        if archived && !children.is_empty() {
            self.archived_with.insert(id, children.to_vec());
        } else {
            self.archived_with.remove(&id);
        }
    }
    /// Settles an archive or restore of `family` (ordered as `archive_family`) after the
    /// store ran: shows exactly the `written` sessions, and records the group
    /// only when the whole of it was saved.
    pub(super) fn finish_archive(
        &mut self,
        family: &[i64],
        written: &[i64],
        complete: bool,
        archived: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for id in family {
            self.archiving.remove(id);
        }
        let split = if archived {
            family.split_last()
        } else {
            family.split_first()
        };
        if let Some((&id, children)) = split
            && complete
        {
            self.remember_archived_with(id, children, archived);
        }
        for &id in written {
            self.apply_archived(id, archived, window, cx);
        }
        self.invalidate_chrome(cx);
        cx.notify();
    }
    /// Shows `id` as archived or restored once the store has saved it.
    pub(super) fn apply_archived(
        &mut self,
        id: i64,
        archived: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(summary) = self.session_summaries.get_mut(&id) {
            summary.archived = archived;
        }
        if let Some(view) = self.sessions.get(&id) {
            view.update(cx, |session, cx| {
                session.summary.archived = archived;
                if archived {
                    session.stop_if_busy(cx);
                }
            });
        }
        self.refresh_session_mark(id, cx);
        if archived && self.workspace.tab_of(id).is_some() {
            self.close_pane(id, window, cx);
        } else {
            self.invalidate_chrome(cx);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};

    /// A parent with two children and a grandchild, plus one unrelated session.
    struct Family {
        parent: i64,
        first: i64,
        second: i64,
        grandchild: i64,
        other: i64,
    }

    fn family(cx: &mut TestAppContext) -> (WindowHandle<Shell>, Family) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let family = window
            .update(cx, |shell, _, cx| {
                let mut ids = Vec::new();
                for _ in 0..5 {
                    let summary = locked(&shell.store)
                        .unwrap()
                        .create("codex", &shell.workspace.project)
                        .unwrap();
                    ids.push(summary.id);
                    shell.add(summary, cx);
                }
                for id in &ids {
                    shell.sessions[id].update(cx, |s, _| s.summary.has_messages = true);
                }
                let family = Family {
                    parent: ids[0],
                    first: ids[1],
                    second: ids[2],
                    grandchild: ids[3],
                    other: ids[4],
                };
                shell.started_by.extend([
                    (family.first, family.parent),
                    (family.second, family.parent),
                    (family.grandchild, family.first),
                ]);
                shell.invalidate_chrome(cx);
                family
            })
            .unwrap();
        cx.run_until_parked();
        (window, family)
    }

    fn archived(window: WindowHandle<Shell>, cx: &mut TestAppContext, id: i64) -> bool {
        window
            .update(cx, |shell, _, cx| {
                let stored = locked(&shell.store)
                    .unwrap()
                    .sessions()
                    .unwrap()
                    .iter()
                    .find(|s| s.id == id)
                    .unwrap()
                    .archived;
                let shown = shell.session_metadata(id, cx).summary.archived;
                assert_eq!(stored, shown, "the store and the sidebar agree on {id}");
                shown
            })
            .unwrap()
    }

    fn toggle(window: WindowHandle<Shell>, cx: &mut TestAppContext, id: i64) {
        window
            .update(cx, |shell, _, cx| shell.toggle_archive(id, cx))
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn archiving_a_parent_archives_everything_it_started(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.parent);
        for id in [f.parent, f.first, f.second, f.grandchild] {
            assert!(archived(window, cx, id), "{id} leaves with its parent");
        }
        assert!(!archived(window, cx, f.other));
        window
            .update(cx, |shell, _, cx| {
                let listed = shell.sidebar_session_ids(cx);
                assert!(listed.contains(&f.other));
                for id in [f.parent, f.first, f.second, f.grandchild] {
                    assert!(!listed.contains(&id));
                }
            })
            .unwrap();
    }

    #[gpui::test]
    fn archiving_a_child_alone_leaves_its_parent_and_siblings(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.second);
        assert!(archived(window, cx, f.second));
        for id in [f.parent, f.first, f.grandchild, f.other] {
            assert!(!archived(window, cx, id));
        }
    }

    #[gpui::test]
    fn archiving_a_middle_session_takes_its_own_children_only(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.first);
        assert!(archived(window, cx, f.first));
        assert!(archived(window, cx, f.grandchild));
        assert!(!archived(window, cx, f.parent));
        assert!(!archived(window, cx, f.second));
    }

    #[gpui::test]
    fn unarchiving_the_parent_restores_the_children_archived_with_it(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.parent);
        toggle(window, cx, f.parent);
        for id in [f.parent, f.first, f.second, f.grandchild] {
            assert!(!archived(window, cx, id), "{id} comes back with its parent");
        }
        window
            .update(cx, |shell, _, _| {
                let (groups, _) = shell.started_groups(&[f.parent, f.first, f.second]);
                assert_eq!(groups[&f.parent], [f.first, f.second]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn unarchiving_the_parent_keeps_a_child_archived_on_its_own(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.second);
        toggle(window, cx, f.parent);
        assert!(archived(window, cx, f.second));
        toggle(window, cx, f.parent);
        assert!(!archived(window, cx, f.parent));
        assert!(!archived(window, cx, f.first));
        assert!(
            archived(window, cx, f.second),
            "archived before the parent, so it stays archived"
        );
    }

    #[gpui::test]
    fn unarchiving_a_child_alone_leaves_its_siblings_archived(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.parent);
        toggle(window, cx, f.first);
        assert!(!archived(window, cx, f.first));
        assert!(archived(window, cx, f.parent));
        assert!(archived(window, cx, f.second));
    }

    #[gpui::test]
    fn archiving_a_parent_closes_the_panes_of_its_children_and_stops_them(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.workspace.open(f.first);
                shell.sessions[&f.first].update(cx, |s, _| s.busy = true);
                assert!(shell.workspace.tab_of(f.first).is_some());
                shell.toggle_archive(f.parent, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.workspace.tab_of(f.first).is_none());
                assert!(shell.sessions[&f.first].read(cx).summary.archived);
                assert!(
                    !shell.sessions[&f.first].read(cx).busy,
                    "a running child is stopped when its parent is archived"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn selecting_a_parent_and_its_child_archives_them_once(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        window
            .update(cx, |shell, window, cx| {
                shell.selected_sessions = HashSet::from([f.parent, f.first]);
                assert!(shell.sidebar_session_ids(cx).contains(&f.first));
                shell.run_session_menu(f.first, "archive", window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        for id in [f.parent, f.first, f.second, f.grandchild] {
            assert!(archived(window, cx, id), "{id}");
        }
        assert!(!archived(window, cx, f.other));
        window
            .update(cx, |shell, _, _| {
                assert!(shell.error.is_none());
                assert!(shell.archiving.is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_child_toggled_on_its_own_is_no_longer_restored_with_its_parent(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.parent);
        toggle(window, cx, f.first);
        toggle(window, cx, f.first);
        assert!(archived(window, cx, f.first));
        toggle(window, cx, f.parent);
        assert!(!archived(window, cx, f.parent));
        assert!(!archived(window, cx, f.second));
        assert!(
            archived(window, cx, f.first),
            "archived on its own after the parent, so the parent's restore leaves it"
        );
    }

    #[test]
    fn writes_stop_at_the_first_failure_and_report_what_was_saved() {
        let mut calls = Vec::new();
        let (written, error) = write_until_failure(&[1, 2, 3], |id| {
            calls.push(id);
            if id == 2 {
                Err("disk full".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(written, [1]);
        assert_eq!(error.as_deref(), Some("disk full"));
        assert_eq!(calls, [1, 2]);
        let (written, error) = write_until_failure(&[1, 2], |_| Ok(()));
        assert_eq!(written, [1, 2]);
        assert!(error.is_none());
    }

    #[gpui::test]
    fn a_partial_archive_shows_exactly_what_was_saved(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        window
            .update(cx, |shell, window, cx| {
                let family = shell.archive_family(f.parent, true, cx);
                shell.archiving.extend(family.iter().copied());
                // The store saved only the first child before it failed.
                shell.finish_archive(&family, &[f.first], false, true, window, cx);
                assert!(shell.archiving.is_empty());
                assert!(shell.session_summaries[&f.first].archived);
                assert!(!shell.session_summaries[&f.parent].archived);
                assert!(!shell.session_summaries[&f.second].archived);
                assert!(
                    !shell.archived_with.contains_key(&f.parent),
                    "an incomplete archive records nothing to undo"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_failed_restore_keeps_the_children_to_restore_on_retry(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.parent);
        window
            .update(cx, |shell, window, cx| {
                let family = shell.archive_family(f.parent, false, cx);
                assert_eq!(family.len(), 4);
                shell.archiving.extend(family.iter().copied());
                // Nothing was saved.
                shell.finish_archive(&family, &[], false, false, window, cx);
            })
            .unwrap();
        toggle(window, cx, f.parent);
        for id in [f.parent, f.first, f.second, f.grandchild] {
            assert!(!archived(window, cx, id), "{id} comes back on the retry");
        }
    }

    #[gpui::test]
    fn a_restore_saves_the_parent_before_its_children(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        toggle(window, cx, f.parent);
        window
            .update(cx, |shell, window, cx| {
                let archive = shell.archive_family(f.parent, true, cx);
                assert_eq!(archive.last(), Some(&f.parent));
                let restore = shell.archive_family(f.parent, false, cx);
                assert_eq!(restore.first(), Some(&f.parent));
                assert_eq!(restore.len(), 4);
                shell.archiving.extend(restore.iter().copied());
                // The store failed right after the parent: no child is restored
                // beside a parent that is still archived.
                shell.finish_archive(&restore, &[f.parent], false, false, window, cx);
                assert!(!shell.session_summaries[&f.parent].archived);
                for id in [f.first, f.second, f.grandchild] {
                    assert!(shell.session_summaries[&id].archived);
                }
                assert!(shell.archived_with.contains_key(&f.parent));
            })
            .unwrap();
    }

    #[gpui::test]
    fn an_unavailable_store_archives_nothing_and_says_so(cx: &mut TestAppContext) {
        let (window, f) = family(cx);
        window
            .update(cx, |shell, _, cx| {
                let store = shell.store.clone();
                let _ = std::thread::spawn(move || {
                    let _held = store.lock().unwrap();
                    panic!("poison the store");
                })
                .join();
                shell.toggle_archive(f.parent, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(
                    shell
                        .error
                        .as_deref()
                        .unwrap()
                        .contains("Could not update archive")
                );
                assert!(shell.archiving.is_empty());
                for id in [f.parent, f.first, f.second, f.grandchild] {
                    assert!(!shell.session_summaries[&id].archived);
                }
            })
            .unwrap();
    }
}
