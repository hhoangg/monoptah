//! The Today filters as pure logic: which rows pass, the button captions, the
//! part that is remembered, and pruning ids that no longer exist. No UI here.
use super::Filters;
use monocode_core::{
    files::path_key,
    today::{Group, Row, Summary, Tag, WorkflowStatus},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Whether a row passes every active filter. Kinds combine with AND. `known` is
/// the current status list (see [`status_passes`]).
pub(super) fn matches(filters: &Filters, row: &Row, known: &[WorkflowStatus]) -> bool {
    let task = &row.task;
    let project = match &filters.project {
        None => true,
        Some(None) => task.project.is_none(),
        Some(Some(path)) => task
            .project
            .as_ref()
            .is_some_and(|p| path_key(p) == path_key(path)),
    };
    let source = match &filters.source {
        None => true,
        Some(wanted) => &task.source_id == wanted,
    };
    let mono = filters
        .mono
        .as_ref()
        .is_none_or(|id| task.mono_id.as_ref() == Some(id));
    project
        && source
        && mono
        && status_passes(filters, &row.status, known)
        && tags_match(filters, row)
}

/// Whether a status (a row's, or a list group or board column's) passes the
/// status filter. Any selected status passes; none selected passes everything.
/// A status that is not in `known` any more (a past day's frozen record of a
/// status deleted since) cannot be selected itself, so it passes when the
/// filter selects the built-in status of the same stage.
pub(super) fn status_passes(
    filters: &Filters,
    status: &WorkflowStatus,
    known: &[WorkflowStatus],
) -> bool {
    if filters.statuses.is_empty() || filters.statuses.contains(&status.id) {
        return true;
    }
    !known.iter().any(|s| s.id == status.id)
        && filters
            .statuses
            .contains(monocode_core::today::statuses::builtin_id(status.stage))
}

/// Whether a task just added by hand (no project, source, Mono or tag) in this
/// status would be hidden by the filters.
pub(super) fn hides_new_task(filters: &Filters, status_id: &str) -> bool {
    filters.project.as_ref().is_some_and(|p| p.is_some())
        || filters.source.as_ref().is_some_and(|s| s.is_some())
        || filters.mono.is_some()
        || !(filters.statuses.is_empty() || filters.statuses.contains(status_id))
        || !tags_pass(filters, &[])
}

/// Tag matching. Nothing selected passes everything.
///
/// Any mode (default): the row has at least one selected tag, or is untagged
/// when No tag is selected.
/// All mode: the row has every selected tag. No tag alone matches untagged rows.
/// No tag together with specific tags matches nothing in All mode, since a row
/// cannot be both untagged and tagged.
pub(super) fn tags_match(filters: &Filters, row: &Row) -> bool {
    let ids: Vec<&str> = row.tags.iter().map(|t| t.id.as_str()).collect();
    tags_pass(filters, &ids)
}
fn tags_pass(filters: &Filters, held: &[&str]) -> bool {
    if filters.tags.is_empty() && !filters.no_tag {
        return true;
    }
    let has = |id: &String| held.contains(&id.as_str());
    let untagged = held.is_empty();
    if filters.match_all {
        if filters.no_tag {
            return filters.tags.is_empty() && untagged;
        }
        filters.tags.iter().all(has)
    } else {
        (filters.no_tag && untagged) || filters.tags.iter().any(has)
    }
}

/// How many kinds of filter are active (project, source, Mono, status, tag).
pub(super) fn active_count(filters: &Filters) -> usize {
    [
        filters.project.is_some(),
        filters.source.is_some(),
        filters.mono.is_some(),
        !filters.statuses.is_empty(),
        !filters.tags.is_empty() || filters.no_tag,
    ]
    .into_iter()
    .filter(|on| *on)
    .count()
}

/// The caption of the active-count in the filters row.
pub(super) fn count_label(count: usize) -> String {
    if count == 1 {
        "1 filter".into()
    } else {
        format!("{count} filters")
    }
}

/// "All statuses", the one name, or "2 statuses".
pub(super) fn status_summary(filters: &Filters, statuses: &[WorkflowStatus]) -> String {
    match filters.statuses.len() {
        0 => "All statuses".into(),
        1 => {
            let id = filters.statuses.iter().next().unwrap();
            statuses
                .iter()
                .find(|s| &s.id == id)
                .map_or_else(|| id.clone(), |s| s.name.clone())
        }
        n => format!("{n} statuses"),
    }
}

/// "All tags", the one name ("No tag" counts as an option), or "3 tags".
pub(super) fn tag_summary(filters: &Filters, tags: &[Tag]) -> String {
    let count = filters.tags.len() + usize::from(filters.no_tag);
    match count {
        0 => "All tags".into(),
        1 if filters.no_tag => "No tag".into(),
        1 => {
            let id = filters.tags.iter().next().unwrap();
            tags.iter()
                .find(|t| &t.id == id)
                .map_or_else(|| id.clone(), |t| t.name.clone())
        }
        n => format!("{n} tags"),
    }
}

/// The open / carried over / to check line, counted over these rows the way the
/// store counts a whole day.
pub(super) fn summary(rows: &[&Row]) -> Summary {
    let mut out = Summary::default();
    for row in rows {
        match row.group {
            Group::CarriedOver => {
                out.open += 1;
                out.carried_over += 1;
            }
            Group::Today => out.open += 1,
            Group::NeedsCheck => out.to_check += 1,
            _ => {}
        }
    }
    out
}

/// The part of the filters that is remembered, as the `today.filters` setting.
/// Every field defaults, so an older or partial value reads safely.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Saved {
    pub statuses: Vec<String>,
    pub tags: Vec<String>,
    pub no_tag: bool,
    pub match_all: bool,
    /// The project path; `project_none` is the No project filter.
    pub project: Option<String>,
    pub project_none: bool,
    /// The source id; `source_none` is the Manual filter.
    pub source: Option<String>,
    pub source_none: bool,
    pub mono: Option<String>,
}
impl Saved {
    pub fn of(filters: &Filters) -> Self {
        Self {
            statuses: filters.statuses.iter().cloned().collect(),
            tags: filters.tags.iter().cloned().collect(),
            no_tag: filters.no_tag,
            match_all: filters.match_all,
            project: filters.project.clone().flatten(),
            project_none: filters.project == Some(None),
            source: filters.source.clone().flatten(),
            source_none: filters.source == Some(None),
            mono: filters.mono.clone(),
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    /// `filters` with the remembered part applied.
    pub fn apply(self, mut filters: Filters) -> Filters {
        filters.statuses = self.statuses.into_iter().collect();
        filters.tags = self.tags.into_iter().collect();
        filters.no_tag = self.no_tag;
        filters.match_all = self.match_all;
        filters.project = match (self.project, self.project_none) {
            (Some(path), _) => Some(Some(path)),
            (None, true) => Some(None),
            (None, false) => None,
        };
        filters.source = match (self.source, self.source_none) {
            (Some(id), _) => Some(Some(id)),
            (None, true) => Some(None),
            (None, false) => None,
        };
        filters.mono = self.mono;
        filters
    }
}

/// Drops status and tag ids that are not in the current lists.
pub(super) fn pruned(mut filters: Filters, statuses: &[WorkflowStatus], tags: &[Tag]) -> Filters {
    let keep_status: BTreeSet<_> = statuses.iter().map(|s| &s.id).collect();
    let keep_tag: BTreeSet<_> = tags.iter().map(|t| &t.id).collect();
    filters.statuses.retain(|id| keep_status.contains(id));
    filters.tags.retain(|id| keep_tag.contains(id));
    filters
}

/// Drops a source filter whose source is gone. Call only once sources are loaded.
pub(super) fn pruned_source(
    mut filters: Filters,
    sources: &[monocode_core::today::Source],
) -> Filters {
    if let Some(Some(id)) = &filters.source
        && !sources.iter().any(|s| &s.id == id)
    {
        filters.source = None;
    }
    filters
}

/// Drops a Mono filter whose Mono is gone. Call only once the roster is known.
pub(super) fn pruned_mono(mut filters: Filters, monos: &[super::MonoInfo]) -> Filters {
    if filters
        .mono
        .as_ref()
        .is_some_and(|id| !monos.iter().any(|m| &m.id == id))
    {
        filters.mono = None;
    }
    filters
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::today::list::testkit::{row_in, task};

    fn tag(id: &str, name: &str) -> Tag {
        Tag {
            id: id.into(),
            name: name.into(),
            color: "#4da3f5".into(),
        }
    }
    fn tagged(status: &str, key: &str, tags: &[&str]) -> Row {
        let mut row = row_in(status, task(key, key), 0);
        row.tags = tags.iter().map(|t| tag(t, t)).collect();
        row
    }
    fn set(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }
    fn keys<'a>(filters: &Filters, rows: &'a [Row]) -> Vec<&'a str> {
        let known = monocode_core::today::statuses::default_statuses();
        rows.iter()
            .filter(|r| matches(filters, r, &known))
            .map(|r| r.task.key.as_str())
            .collect()
    }
    fn rows() -> Vec<Row> {
        vec![
            tagged("todo", "a", &["x"]),
            tagged("in_progress", "b", &["x", "y"]),
            tagged("done", "c", &[]),
            tagged("todo", "d", &["y"]),
        ]
    }

    #[test]
    fn no_filter_passes_everything() {
        assert_eq!(keys(&Filters::default(), &rows()), ["a", "b", "c", "d"]);
    }
    #[test]
    fn several_statuses_pass_any_of_them() {
        let f = Filters {
            statuses: set(&["todo", "done"]),
            ..Default::default()
        };
        assert_eq!(keys(&f, &rows()), ["a", "c", "d"]);
    }
    #[test]
    fn any_tag_mode_passes_rows_with_one_of_the_tags() {
        let f = Filters {
            tags: set(&["x", "y"]),
            ..Default::default()
        };
        assert_eq!(keys(&f, &rows()), ["a", "b", "d"]);
        let only_x = Filters {
            tags: set(&["x"]),
            ..Default::default()
        };
        assert_eq!(keys(&only_x, &rows()), ["a", "b"]);
    }
    #[test]
    fn all_tag_mode_needs_every_selected_tag() {
        let f = Filters {
            tags: set(&["x", "y"]),
            match_all: true,
            ..Default::default()
        };
        assert_eq!(keys(&f, &rows()), ["b"]);
    }
    #[test]
    fn no_tag_alone_passes_untagged_rows_in_both_modes() {
        for match_all in [false, true] {
            let f = Filters {
                no_tag: true,
                match_all,
                ..Default::default()
            };
            assert_eq!(keys(&f, &rows()), ["c"]);
        }
    }
    #[test]
    fn no_tag_with_tags_is_or_in_any_mode_and_nothing_in_all_mode() {
        let any = Filters {
            no_tag: true,
            tags: set(&["y"]),
            ..Default::default()
        };
        assert_eq!(keys(&any, &rows()), ["b", "c", "d"]);
        let all = Filters {
            match_all: true,
            ..any
        };
        assert!(keys(&all, &rows()).is_empty());
    }
    #[test]
    fn kinds_combine_with_and() {
        let mut rows = rows();
        rows[0].task.project = Some("/p".into());
        rows[3].task.project = Some("/p".into());
        let f = Filters {
            project: Some(Some("/p".into())),
            statuses: set(&["todo"]),
            tags: set(&["y"]),
            ..Default::default()
        };
        assert_eq!(keys(&f, &rows), ["d"]);
    }
    #[test]
    fn a_gone_status_passes_through_the_builtin_of_its_stage() {
        let known = monocode_core::today::statuses::default_statuses();
        let mut gone = known[1].clone();
        gone.id = "custom".into();
        let mut row = tagged("in_progress", "g", &[]);
        row.status = gone;
        let by = |ids: &[&str]| Filters {
            statuses: set(ids),
            ..Default::default()
        };
        assert!(matches(&by(&["in_progress"]), &row, &known));
        assert!(!matches(&by(&["todo"]), &row, &known));
        // Still known: only its own id counts.
        let mut with = known.clone();
        with.push(row.status.clone());
        assert!(!matches(&by(&["in_progress"]), &row, &with));
        assert!(matches(&by(&["custom"]), &row, &with));
    }
    #[test]
    fn match_all_alone_is_not_a_filter() {
        let f = Filters {
            match_all: true,
            ..Default::default()
        };
        assert_eq!(active_count(&f), 0);
        assert_eq!(keys(&f, &rows()).len(), 4);
    }
    #[test]
    fn a_new_manual_task_is_hidden_by_filters_it_cannot_pass() {
        let mut f = Filters::default();
        assert!(!hides_new_task(&f, "todo"));
        f.project = Some(None);
        f.source = Some(None);
        assert!(!hides_new_task(&f, "todo"), "No project and Manual fit");
        f.statuses = set(&["done"]);
        assert!(hides_new_task(&f, "todo"));
        assert!(!hides_new_task(&f, "done"));
        f.statuses.clear();
        f.tags = set(&["x"]);
        assert!(hides_new_task(&f, "todo"));
        f.no_tag = true;
        assert!(
            !hides_new_task(&f, "todo"),
            "No tag or x fits an untagged task"
        );
        f.match_all = true;
        assert!(hides_new_task(&f, "todo"));
        f.tags.clear();
        assert!(!hides_new_task(&f, "todo"));
        f.mono = Some("m".into());
        assert!(hides_new_task(&f, "todo"));
    }
    #[test]
    fn the_active_count_counts_kinds_not_values() {
        let mut f = Filters::default();
        assert_eq!(active_count(&f), 0);
        f.statuses = set(&["todo", "done"]);
        f.no_tag = true;
        f.tags = set(&["x"]);
        assert_eq!(active_count(&f), 2);
        f.mono = Some("m".into());
        assert_eq!(active_count(&f), 3);
        assert_eq!(count_label(1), "1 filter");
        assert_eq!(count_label(3), "3 filters");
    }
    #[test]
    fn summaries_name_one_and_count_many() {
        let statuses = monocode_core::today::statuses::default_statuses();
        let tags = vec![tag("x", "Urgent"), tag("y", "Home")];
        let mut f = Filters::default();
        assert_eq!(status_summary(&f, &statuses), "All statuses");
        assert_eq!(tag_summary(&f, &tags), "All tags");
        f.statuses = set(&["todo"]);
        assert_eq!(status_summary(&f, &statuses), "To do");
        f.statuses = set(&["todo", "done"]);
        assert_eq!(status_summary(&f, &statuses), "2 statuses");
        f.tags = set(&["x"]);
        assert_eq!(tag_summary(&f, &tags), "Urgent");
        f.no_tag = true;
        assert_eq!(tag_summary(&f, &tags), "2 tags");
        f.tags.clear();
        assert_eq!(tag_summary(&f, &tags), "No tag");
        f.tags = set(&["x", "y"]);
        assert_eq!(tag_summary(&f, &tags), "3 tags");
    }
    #[test]
    fn the_saved_value_round_trips_and_reads_partial_or_bad_input() {
        let f = Filters {
            statuses: set(&["todo"]),
            tags: set(&["x"]),
            no_tag: true,
            match_all: true,
            mono: Some("m".into()),
            ..Default::default()
        };
        let back = Saved::parse(&Saved::of(&f).to_json()).unwrap();
        let applied = back.apply(Filters::default());
        assert_eq!(applied.statuses, f.statuses);
        assert_eq!(applied.tags, f.tags);
        assert!(applied.no_tag && applied.match_all);
        assert_eq!(applied.mono.as_deref(), Some("m"));
        let wide = Filters {
            project: Some(None),
            source: Some(Some("s".into())),
            ..Default::default()
        };
        let back = Saved::parse(&Saved::of(&wide).to_json()).unwrap();
        let applied = back.apply(Filters::default());
        assert_eq!(
            (applied.project, applied.source),
            (wide.project, wide.source)
        );
        let partial = Saved::parse(r#"{"statuses":["done"]}"#).unwrap();
        assert_eq!(partial.statuses, ["done"]);
        assert!(!partial.no_tag && partial.tags.is_empty());
        assert!(Saved::parse("not json").is_none());
        assert_eq!(Saved::parse("{}").unwrap(), Saved::default());
    }
    #[test]
    fn unknown_ids_are_pruned() {
        let statuses = monocode_core::today::statuses::default_statuses();
        let f = Filters {
            statuses: set(&["todo", "ghost"]),
            tags: set(&["x", "gone"]),
            ..Default::default()
        };
        let f = pruned(f, &statuses, &[tag("x", "X")]);
        assert_eq!(f.statuses, set(&["todo"]));
        assert_eq!(f.tags, set(&["x"]));
    }
    #[test]
    fn the_summary_counts_by_group() {
        use monocode_core::today::Group;
        let mut a = tagged("todo", "a", &[]);
        a.group = Group::CarriedOver;
        let mut b = tagged("todo", "b", &[]);
        b.group = Group::NeedsCheck;
        let c = tagged("todo", "c", &[]);
        let s = summary(&[&a, &b, &c]);
        assert_eq!((s.open, s.carried_over, s.to_check), (2, 1, 1));
    }
}
