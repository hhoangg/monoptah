//! Which of an Inbox item's related sessions are running a turn right now.
use monocode_core::session_groups::WorkItem;
use monocode_core::store::Session;
use std::collections::HashSet;

/// Whether a session's linked work item is the Inbox item `item`. The URL is
/// the identity; kind, repo and number only stand in for it when the number is
/// a real one (a ClickUp task without a custom id has number 0, and would
/// otherwise match every such task in its space).
pub(super) fn same_work_item(linked: &WorkItem, item: &WorkItem) -> bool {
    if !linked.url.is_empty() && linked.url == item.url {
        return true;
    }
    linked.number != 0
        && linked.kind == item.kind
        && linked.number == item.number
        && linked.repo.eq_ignore_ascii_case(&item.repo)
}

/// How many of the `related` sessions are in the `busy` set.
pub(super) fn running_in(related: &[&Session], busy: &HashSet<i64>) -> usize {
    running_count(related.iter().map(|s| s.id), busy)
}

/// How many of `related` session ids are in the `busy` set.
fn running_count(related: impl IntoIterator<Item = i64>, busy: &HashSet<i64>) -> usize {
    related.into_iter().filter(|id| busy.contains(id)).count()
}

/// Row badge text and accessible name for an item's related threads.
pub(super) fn related_threads_label(count: usize, running: usize) -> String {
    let base = format!(
        "{count} related thread{}",
        if count == 1 { "" } else { "s" }
    );
    if running > 0 {
        format!("{base}, {running} running")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn counts_only_the_related_sessions_that_are_busy() {
        let related = [1, 2, 3];
        assert_eq!(running_count(related, &HashSet::from([2, 9])), 1);
        assert_eq!(running_count(related, &HashSet::from([1, 2, 3])), 3);
    }

    #[test]
    fn is_zero_without_a_busy_set_or_without_related_sessions() {
        assert_eq!(running_count([1, 2, 3], &HashSet::new()), 0);
        assert_eq!(running_count([], &HashSet::from([1])), 0);
    }

    #[test]
    fn pluralises_the_thread_count() {
        assert_eq!(related_threads_label(1, 0), "1 related thread");
        assert_eq!(related_threads_label(3, 0), "3 related threads");
    }

    #[test]
    fn appends_how_many_are_running() {
        assert_eq!(related_threads_label(1, 1), "1 related thread, 1 running");
        assert_eq!(related_threads_label(3, 2), "3 related threads, 2 running");
    }

    fn work(kind: &str, repo: &str, number: u64, url: &str) -> WorkItem {
        WorkItem {
            kind: kind.into(),
            repo: repo.into(),
            number,
            url: url.into(),
        }
    }

    #[test]
    fn the_url_identifies_a_work_item() {
        let linked = work("issue", "acme/web", 9, "https://x/9");
        assert!(same_work_item(
            &linked,
            &work("pr", "other", 1, "https://x/9")
        ));
        assert!(!same_work_item(
            &linked,
            &work("issue", "acme/web", 10, "https://x/10")
        ));
    }

    #[test]
    fn kind_repo_and_number_stand_in_for_a_missing_url() {
        let linked = work("issue", "Acme/Web", 9, "");
        assert!(same_work_item(
            &linked,
            &work("issue", "acme/web", 9, "https://x/9")
        ));
        assert!(!same_work_item(
            &linked,
            &work("issue", "acme/web", 8, "https://x/8")
        ));
    }

    #[test]
    fn number_zero_never_matches_on_kind_and_repo() {
        let linked = work("clickup", "Design", 0, "https://c/a");
        assert!(!same_work_item(
            &linked,
            &work("clickup", "Design", 0, "https://c/b")
        ));
        assert!(!same_work_item(
            &work("clickup", "Design", 0, ""),
            &work("clickup", "Design", 0, "")
        ));
        assert!(same_work_item(
            &linked,
            &work("clickup", "Design", 0, "https://c/a")
        ));
    }
}
