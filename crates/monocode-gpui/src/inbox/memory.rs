//! What the Inbox list shows for each source, kept while the app is open.
//! In memory only: a restart starts from a clean list.
use std::collections::HashMap;

/// Search, selection, scroll and page size of one source's list.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ViewMemory {
    pub search: String,
    pub selected_key: Option<String>,
    /// Distance scrolled from the top of the list, in pixels.
    pub scroll: f32,
    /// Rows mounted when the source was left. The native list shows every row,
    /// so nothing sets it yet; it keeps the shape of the desktop app's memory.
    #[allow(dead_code)]
    pub list_limit: Option<usize>,
}

#[derive(Default)]
pub(super) struct ViewMemories(HashMap<String, ViewMemory>);

impl ViewMemories {
    pub fn get(&self, source: &str) -> ViewMemory {
        self.0.get(source).cloned().unwrap_or_default()
    }

    pub fn set(&mut self, source: &str, mut view: ViewMemory) {
        if !view.scroll.is_finite() || view.scroll < 0. {
            view.scroll = 0.;
        }
        self.0.insert(source.to_string(), view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(search: &str, key: Option<&str>, scroll: f32, limit: Option<usize>) -> ViewMemory {
        ViewMemory {
            search: search.into(),
            selected_key: key.map(Into::into),
            scroll,
            list_limit: limit,
        }
    }

    #[test]
    fn every_source_starts_with_an_empty_view() {
        assert_eq!(
            ViewMemories::default().get("github"),
            view("", None, 0., None)
        );
    }

    #[test]
    fn the_view_of_each_source_is_kept_apart() {
        let mut views = ViewMemories::default();
        views.set(
            "github",
            view("fix", Some("github:acme/web#pr:7"), 420., Some(64)),
        );
        views.set("linear", view("", Some("linear:ENG-1"), 0., Some(32)));
        assert_eq!(
            views.get("github"),
            view("fix", Some("github:acme/web#pr:7"), 420., Some(64))
        );
        assert_eq!(
            views.get("linear").selected_key.as_deref(),
            Some("linear:ENG-1")
        );
        assert_eq!(views.get("jira").selected_key, None);
    }

    #[test]
    fn a_returned_view_is_a_copy_so_callers_cannot_change_the_store() {
        let mut views = ViewMemories::default();
        views.set("github", view("a", None, 1., None));
        let mut copy = views.get("github");
        copy.search = "changed".into();
        assert_eq!(views.get("github").search, "a");
    }

    #[test]
    fn a_bad_scroll_offset_is_clamped_to_zero() {
        let mut views = ViewMemories::default();
        views.set("gitlab", view("", None, -20., None));
        assert_eq!(views.get("gitlab").scroll, 0.);
        views.set("gitlab", view("", None, f32::NAN, None));
        assert_eq!(views.get("gitlab").scroll, 0.);
        views.set("gitlab", view("", None, f32::INFINITY, None));
        assert_eq!(views.get("gitlab").scroll, 0.);
    }

    #[test]
    fn a_new_store_remembers_nothing_like_after_a_restart() {
        let mut views = ViewMemories::default();
        views.set("clickup", view("x", Some("k"), 5., Some(32)));
        assert_eq!(ViewMemories::default().get("clickup").search, "");
    }
}
