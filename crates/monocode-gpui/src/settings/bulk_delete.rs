//! Pure state for deleting several worktrees from Settings, one after another.
//! The gpui wiring in `management.rs` feeds it each result and emits the next
//! `SettingsEvent::RemoveWorktree`, so the sequencing can be tested without a window.
use super::data;
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Item {
    pub path: String,
    /// Branch name, or the folder name for a detached worktree.
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Failure {
    pub item: Item,
    pub error: String,
}
#[derive(Debug)]
pub(super) struct BulkDelete {
    pending: VecDeque<Item>,
    current: Option<Item>,
    total: usize,
    deleted: Vec<Item>,
    failures: Vec<Failure>,
}
impl BulkDelete {
    pub fn new(items: Vec<Item>) -> Self {
        Self {
            total: items.len(),
            pending: items.into(),
            current: None,
            deleted: Vec::new(),
            failures: Vec::new(),
        }
    }
    /// Takes the first item. Call once; later items come from `record`.
    pub fn start(&mut self) -> Option<Item> {
        if self.current.is_some() {
            return None;
        }
        self.current = self.pending.pop_front();
        self.current.clone()
    }
    /// Records the outcome of the current item and moves to the next one.
    /// Failures never stop the run.
    pub fn record(&mut self, result: Result<(), String>) -> Option<Item> {
        let item = self.current.take()?;
        match result {
            Ok(()) => self.deleted.push(item),
            Err(error) => self.failures.push(Failure { item, error }),
        }
        self.current = self.pending.pop_front();
        self.current.clone()
    }
    pub fn current(&self) -> Option<&Item> {
        self.current.as_ref()
    }
    pub fn running(&self) -> bool {
        self.current.is_some()
    }
    /// Every item has been tried (also true for an empty run).
    pub fn finished(&self) -> bool {
        self.current.is_none() && self.pending.is_empty()
    }
    pub fn total(&self) -> usize {
        self.total
    }
    /// 1-based position of the item being deleted.
    pub fn position(&self) -> usize {
        (self.deleted.len() + self.failures.len() + 1).min(self.total)
    }
    pub fn deleted(&self) -> &[Item] {
        &self.deleted
    }
    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }
}

/// Locked and detached worktrees cannot be deleted from Settings.
pub(super) fn selectable(tree: &data::Tree) -> bool {
    !tree.locked && tree.tree.branch.is_some()
}
pub(super) fn item(tree: &data::Tree) -> Item {
    let path = tree.tree.path.clone();
    let label = tree.tree.branch.clone().unwrap_or_else(|| {
        std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone())
    });
    Item { path, label }
}
/// Paths that bulk actions may touch, in list order.
pub(super) fn selectable_paths(trees: &[data::Tree]) -> Vec<String> {
    trees
        .iter()
        .filter(|t| selectable(t))
        .map(|t| t.tree.path.clone())
        .collect()
}
/// Labels of trees whose changes would be lost (`Some(true)`) and of trees whose
/// status could not be read (`None`), which still warrant a warning.
pub(super) fn split_dirty(trees: &[data::Tree]) -> (Vec<String>, Vec<String>) {
    let label = |t: &data::Tree| item(t).label;
    (
        trees
            .iter()
            .filter(|t| t.dirty == Some(true))
            .map(label)
            .collect(),
        trees
            .iter()
            .filter(|t| t.dirty.is_none())
            .map(label)
            .collect(),
    )
}
/// Dialog title once a run has finished with failures.
pub(super) fn result_title(deleted: usize, total: usize) -> String {
    format!("{deleted} of {total} worktrees deleted")
}
/// Selects every path, or clears them when all are already selected.
pub(super) fn toggle_all(selected: &mut HashSet<String>, paths: &[String]) {
    if !paths.is_empty() && paths.iter().all(|p| selected.contains(p)) {
        for path in paths {
            selected.remove(path);
        }
    } else {
        selected.extend(paths.iter().cloned());
    }
}
pub(super) fn toggle(selected: &mut HashSet<String>, path: &str) {
    if !selected.remove(path) {
        selected.insert(path.to_owned());
    }
}
/// Drops selected paths that are no longer selectable in the list.
pub(super) fn prune(selected: &mut HashSet<String>, selectable_paths: &[String]) {
    selected.retain(|p| selectable_paths.contains(p));
}

#[cfg(test)]
mod tests {
    use super::*;
    use monocode_core::git::Worktree;

    fn tree(path: &str, branch: Option<&str>, locked: bool) -> data::Tree {
        data::Tree {
            tree: Worktree {
                path: path.into(),
                branch: branch.map(Into::into),
                ..Default::default()
            },
            locked,
            dirty: Some(false),
            unpushed: 0,
        }
    }
    fn items(names: &[&str]) -> Vec<Item> {
        names
            .iter()
            .map(|n| Item {
                path: format!("/w/{n}"),
                label: (*n).into(),
            })
            .collect()
    }

    #[test]
    fn runs_items_in_order_and_finishes() {
        let mut run = BulkDelete::new(items(&["a", "b", "c"]));
        assert!(!run.running() && !run.finished());
        assert_eq!(run.start().unwrap().label, "a");
        assert_eq!((run.position(), run.total()), (1, 3));
        assert_eq!(run.record(Ok(())).unwrap().label, "b");
        assert_eq!(run.position(), 2);
        assert_eq!(run.record(Ok(())).unwrap().label, "c");
        assert_eq!(run.position(), 3);
        assert!(run.record(Ok(())).is_none());
        assert!(run.finished() && !run.running());
        assert_eq!(run.deleted().len(), 3);
        assert!(run.failures().is_empty());
        assert_eq!(run.position(), 3);
    }
    #[test]
    fn keeps_going_after_a_failure_and_reports_it() {
        let mut run = BulkDelete::new(items(&["a", "b", "c"]));
        run.start();
        assert_eq!(run.record(Err("in use".into())).unwrap().label, "b");
        assert_eq!(run.record(Ok(())).unwrap().label, "c");
        assert!(run.record(Err("dirty".into())).is_none());
        assert!(run.finished());
        assert_eq!(
            run.deleted()
                .iter()
                .map(|i| &i.label[..])
                .collect::<Vec<_>>(),
            ["b"]
        );
        assert_eq!(run.failures().len(), 2);
        assert_eq!(run.failures()[0].item.label, "a");
        assert_eq!(run.failures()[0].error, "in use");
        assert_eq!(run.failures()[1].item.path, "/w/c");
    }
    #[test]
    fn record_without_a_current_item_is_ignored() {
        let mut run = BulkDelete::new(items(&["a"]));
        assert!(run.record(Ok(())).is_none());
        assert!(run.deleted().is_empty() && run.failures().is_empty());
        run.start();
        run.record(Ok(()));
        assert!(run.record(Err("late".into())).is_none());
        assert!(run.failures().is_empty());
        assert!(run.start().is_none());
    }
    #[test]
    fn empty_run_is_finished() {
        let mut run = BulkDelete::new(Vec::new());
        assert!(run.finished());
        assert!(run.start().is_none());
        assert_eq!(run.position(), 0);
    }
    #[test]
    fn locked_and_detached_trees_are_not_selectable() {
        assert!(selectable(&tree("/w/a", Some("a"), false)));
        assert!(!selectable(&tree("/w/a", Some("a"), true)));
        assert!(!selectable(&tree("/w/a", None, false)));
    }
    #[test]
    fn item_label_falls_back_to_folder_name() {
        assert_eq!(item(&tree("/w/a", Some("feat/a"), false)).label, "feat/a");
        assert_eq!(item(&tree("/w/detached", None, false)).label, "detached");
    }
    #[test]
    fn toggle_all_selects_then_clears() {
        let paths = vec!["/w/a".to_owned(), "/w/b".to_owned()];
        let mut sel = HashSet::new();
        toggle(&mut sel, "/w/a");
        toggle_all(&mut sel, &paths);
        assert_eq!(sel.len(), 2);
        toggle_all(&mut sel, &paths);
        assert!(sel.is_empty());
        toggle_all(&mut sel, &[]);
        assert!(sel.is_empty());
    }
    #[test]
    fn select_all_over_a_mixed_list_never_selects_skipped_trees() {
        let trees = vec![
            tree("/w/a", Some("a"), false),
            tree("/w/locked", Some("locked"), true),
            tree("/w/detached", None, false),
            tree("/w/b", Some("b"), false),
        ];
        let paths = selectable_paths(&trees);
        assert_eq!(paths, ["/w/a", "/w/b"]);
        let mut sel = HashSet::new();
        toggle_all(&mut sel, &paths);
        assert_eq!(sel.len(), 2);
        assert!(!sel.contains("/w/locked") && !sel.contains("/w/detached"));
    }
    #[test]
    fn unknown_status_is_listed_apart_from_uncommitted_changes() {
        let mut dirty = tree("/w/dirty", Some("dirty"), false);
        dirty.dirty = Some(true);
        let mut unknown = tree("/w/unknown", Some("unknown"), false);
        unknown.dirty = None;
        let clean = tree("/w/clean", Some("clean"), false);
        let (changed, unsure) = split_dirty(&[dirty, clean, unknown]);
        assert_eq!(changed, ["dirty"]);
        assert_eq!(unsure, ["unknown"]);
    }
    #[test]
    fn result_title_counts_deleted_worktrees() {
        assert_eq!(result_title(2, 5), "2 of 5 worktrees deleted");
        assert_eq!(result_title(0, 1), "0 of 1 worktrees deleted");
    }
    #[test]
    fn prune_drops_vanished_paths() {
        let mut sel: HashSet<String> = ["/w/a", "/w/gone"].iter().map(|s| s.to_string()).collect();
        prune(&mut sel, &["/w/a".to_owned(), "/w/b".to_owned()]);
        assert_eq!(sel.len(), 1);
        assert!(sel.contains("/w/a"));
    }
}
