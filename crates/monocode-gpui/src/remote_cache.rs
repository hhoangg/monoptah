//! The last listing and git snapshot of recent cloud workspaces, in memory only.
//!
//! The panels show this data, dimmed and read-only, while the tunnel to a workspace is down, and
//! open instantly when the user switches between sessions. It holds file names and git status
//! (paths, statuses and line counts), never file contents, tokens or environment values, and it
//! is never written to disk. Entries are keyed by the exact remote root (`remote://env/path`), so
//! the tree of one workspace is never shown for another, and local checkouts are never stored
//! here: they keep `ProjectGit`'s own memory.
use monocode_core::{files::DirEntry, scm::DiffIndex};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

/// How many remote roots are remembered; the least recently used one goes first.
pub(crate) const CAPACITY: usize = 16;
/// A snapshot with more files than this is not kept: the memory is bounded by roots, and by this.
const MAX_INDEX_FILES: usize = 20_000;

/// What is remembered for one remote root.
#[derive(Clone, Default)]
pub(crate) struct Entry {
    /// Each folder the Explorer listed, by full path.
    pub dirs: HashMap<String, Vec<DirEntry>>,
    /// The Explorer's open folders and selection, so the tree comes back as it was left.
    pub expanded: Option<HashSet<String>>,
    pub selected: Option<String>,
    /// The git snapshot of the root.
    pub index: Option<DiffIndex>,
    /// The last time a read from the live workspace was stored.
    pub updated: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct RemoteCache {
    /// Least recently used first.
    entries: Vec<(String, Entry)>,
}

/// Whether `path` is `root` or inside it. A sibling that shares the prefix is not inside.
pub(crate) fn within(root: &str, path: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

impl RemoteCache {
    /// Only a `remote://` root is cached.
    fn cacheable(root: &str) -> bool {
        root.starts_with("remote://")
    }
    /// The entry of `root`, made the most recently used; evicts the oldest root over capacity.
    fn touch(&mut self, root: &str) -> Option<&mut Entry> {
        if !Self::cacheable(root) {
            return None;
        }
        let at = match self.entries.iter().position(|(key, _)| key == root) {
            Some(at) => at,
            None => {
                self.entries.push((root.to_owned(), Entry::default()));
                self.entries.len() - 1
            }
        };
        let last = self.entries.remove(at);
        self.entries.push(last);
        if self.entries.len() > CAPACITY {
            self.entries.remove(0);
        }
        self.entries.last_mut().map(|(_, entry)| entry)
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
    /// The roots, least recently used first.
    #[cfg(test)]
    pub(crate) fn roots(&self) -> Vec<&str> {
        self.entries.iter().map(|(key, _)| key.as_str()).collect()
    }
    /// What is remembered for `root`, without counting as a use.
    pub(crate) fn peek(&self, root: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|(key, _)| key == root)
            .map(|(_, entry)| entry)
    }
    /// What is remembered for `root`, counting as a use.
    pub(crate) fn get(&mut self, root: &str) -> Option<Entry> {
        self.entries
            .iter()
            .any(|(key, _)| key == root)
            .then_some(())?;
        self.touch(root).map(|entry| entry.clone())
    }
    /// Stores the Explorer's state for `root`. Nothing is stored for an empty tree.
    pub(crate) fn put_tree(
        &mut self,
        root: &str,
        dirs: HashMap<String, Vec<DirEntry>>,
        expanded: HashSet<String>,
        selected: Option<String>,
        now: Instant,
    ) {
        if dirs.is_empty() {
            return;
        }
        if let Some(entry) = self.touch(root) {
            entry.dirs = dirs;
            entry.expanded = Some(expanded);
            entry.selected = selected;
            entry.updated = Some(now);
        }
    }
    /// Stores a git snapshot read from the live workspace.
    pub(crate) fn put_index(&mut self, root: &str, index: &DiffIndex, now: Instant) {
        if index.files.len() > MAX_INDEX_FILES {
            return;
        }
        if let Some(entry) = self.touch(root) {
            entry.index = Some(index.clone());
            entry.updated = Some(now);
        }
    }
    /// Records that the live workspace was read for `root` just now.
    pub(crate) fn confirm(&mut self, root: &str, now: Instant) {
        if let Some(entry) = self.touch(root) {
            entry.updated = Some(now);
        }
    }
    /// How long ago the live workspace was last read for `root`.
    pub(crate) fn age(&self, root: &str, now: Instant) -> Option<Duration> {
        let at = self.peek(root)?.updated?;
        Some(now.saturating_duration_since(at))
    }
}

/// "last updated 5 min ago", for the strip above dimmed panels.
pub(crate) fn age_hint(age: Duration) -> String {
    let secs = age.as_secs();
    let ago = match secs {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    };
    format!("last updated {ago}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(n: usize) -> String {
        format!("remote://env-{n}/workspaces/widgets")
    }
    fn dirs_of(root: &str, names: &[&str]) -> HashMap<String, Vec<DirEntry>> {
        let entries = names
            .iter()
            .map(|name| DirEntry {
                name: (*name).into(),
                path: format!("{root}/{name}"),
                is_dir: false,
                ignored: false,
            })
            .collect();
        HashMap::from([(root.to_owned(), entries)])
    }
    fn put(cache: &mut RemoteCache, root: &str, now: Instant) {
        cache.put_tree(root, dirs_of(root, &["a.rs"]), HashSet::new(), None, now);
    }

    #[test]
    fn the_least_recently_used_root_is_evicted_over_sixteen() {
        let now = Instant::now();
        let mut cache = RemoteCache::default();
        for n in 0..CAPACITY {
            put(&mut cache, &root(n), now);
        }
        assert_eq!(cache.len(), CAPACITY);
        // Reading root 0 makes it the most recent, so root 1 is the oldest.
        assert!(cache.get(&root(0)).is_some());
        put(&mut cache, &root(99), now);
        assert_eq!(cache.len(), CAPACITY);
        assert!(cache.peek(&root(1)).is_none(), "the oldest root is gone");
        assert!(cache.peek(&root(0)).is_some() && cache.peek(&root(99)).is_some());
        assert_eq!(cache.roots().last().copied(), Some(root(99).as_str()));
        // A write also counts as a use.
        put(&mut cache, &root(2), now);
        put(&mut cache, &root(98), now);
        assert!(cache.peek(&root(2)).is_some() && cache.peek(&root(3)).is_none());
    }

    #[test]
    fn a_root_never_answers_for_another_root() {
        let now = Instant::now();
        let mut cache = RemoteCache::default();
        let (a, b) = (root(1), format!("{}-wt", root(1)));
        put(&mut cache, &a, now);
        assert!(cache.get(&b).is_none());
        assert!(cache.get("remote://env-1/workspaces").is_none());
        assert!(cache.get(&a).is_some());
        assert!(within(&a, &format!("{a}/src")) && within(&a, &a));
        assert!(
            !within(&a, &b),
            "a sibling sharing the prefix is not inside"
        );
    }

    #[test]
    fn local_roots_and_empty_trees_are_never_stored() {
        let now = Instant::now();
        let mut cache = RemoteCache::default();
        put(&mut cache, "/work/monocode", now);
        cache.put_tree(&root(1), HashMap::new(), HashSet::new(), None, now);
        cache.put_index("/work/monocode", &DiffIndex::default(), now);
        cache.confirm("~", now);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn a_snapshot_over_the_file_limit_is_not_kept() {
        let now = Instant::now();
        let mut cache = RemoteCache::default();
        let mut index = DiffIndex::default();
        index.files = (0..=MAX_INDEX_FILES)
            .map(|n| monocode_core::scm::ChangedFile {
                path: format!("/r/{n}"),
                relative: n.to_string(),
                status: "modified".into(),
                additions: 0,
                deletions: 0,
                staged: false,
                unstaged: true,
            })
            .collect();
        cache.put_index(&root(1), &index, now);
        assert_eq!(cache.len(), 0);
        index.files.truncate(2);
        cache.put_index(&root(1), &index, now);
        assert_eq!(cache.peek(&root(1)).unwrap().index.as_ref(), Some(&index));
    }

    #[test]
    fn the_age_follows_the_last_stored_read() {
        let t0 = Instant::now();
        let mut cache = RemoteCache::default();
        assert_eq!(cache.age(&root(1), t0), None);
        cache.confirm(&root(1), t0);
        assert_eq!(
            cache.age(&root(1), t0 + Duration::from_secs(300)),
            Some(Duration::from_secs(300))
        );
        assert_eq!(age_hint(Duration::from_secs(5)), "last updated just now");
        assert_eq!(age_hint(Duration::from_secs(300)), "last updated 5 min ago");
        assert_eq!(age_hint(Duration::from_secs(7300)), "last updated 2 h ago");
        assert_eq!(
            age_hint(Duration::from_secs(200_000)),
            "last updated 2 d ago"
        );
    }
}
