//! Pure resolution of a file link as written in a chat: `~`, `file://`, `%` escapes, a `:12:3` or
//! `#L12` location, a join onto the chat's folder, and the re-rooting among a Mono's project
//! folders. It lives here so the app's click handling (`file_links.rs` in the GPUI crate) and the
//! remote `chat.mentionedFile` method decide the same path for the same text.
use std::path::{Path, PathBuf};

use super::{document, path_key, remote::is_remote};

/// The rule an agent's system prompt carries so the files it names can be opened from the chat
/// (a bare name is only looked for in the places of [`search_roots`]).
pub const MENTION_RULE: &str = "When you mention a file, write its absolute path if it lies outside the project you are working in, and never mention a file that does not exist yet as a link or path.";

/// The path of a file link without its `file://` prefix, percent escapes and line/column suffix,
/// and the one-based location when it had one. Nothing is resolved against a folder.
pub fn split_location(raw: &str) -> (String, Option<(usize, usize)>) {
    let mut path = raw.strip_prefix("file://").unwrap_or(raw).to_owned();
    let mut location = None;
    // `#L12`, `#L12C3`, `#L12-L20`, `#L12C3-L20C5`: the start of the range.
    let hash_line = path.rsplit_once("#L").and_then(|(file, anchor)| {
        let start = anchor.split('-').next().unwrap_or("");
        let (line, column) = match start.split_once('C') {
            Some((line, column)) => (line, Some(column.parse::<usize>().ok()?)),
            None => (start, None),
        };
        Some((file.to_owned(), line.parse::<usize>().ok()?, column))
    });
    if let Some((file, line, column)) = hash_line {
        location = Some((line.max(1), column.unwrap_or(1).max(1)));
        path = file;
    } else {
        let colon = path.rsplit_once(':').and_then(|(prefix, number)| {
            let last = number.parse::<usize>().ok()?;
            Some((prefix.to_owned(), last))
        });
        if let Some((prefix, last)) = colon {
            let inner = prefix.rsplit_once(':').and_then(|(file, line)| {
                let line = line.parse::<usize>().ok()?;
                Some((file.to_owned(), line))
            });
            if let Some((file, line)) = inner {
                location = Some((line.max(1), last.max(1)));
                path = file;
            } else {
                location = Some((last.max(1), 1));
                path = prefix;
            }
        }
    }
    if raw.starts_with("file://") || raw.contains('%') {
        path = decode_percent(&path);
    }
    (path, location)
}

/// `%XX` escapes decoded; a malformed escape stays as written.
fn decode_percent(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut decoded = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%' && at + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[at + 1..at + 3]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                at += 3;
            }
            None => {
                decoded.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// A link resolved against `cwd` without touching the disk, and its location.
pub fn file_location(raw: &str, cwd: &str) -> (String, Option<(usize, usize)>) {
    let (path, location) = split_location(raw);
    let path = document::resolve_path(Path::new(&path), Path::new(cwd));
    (path.to_string_lossy().into_owned(), location)
}

/// Whether the link names a plain relative path (not `~`, not `file://`, not absolute), and so
/// belongs to whichever project folder has it.
pub fn written_relative(raw: &str) -> Option<String> {
    let (written, _) = split_location(raw);
    (!written.starts_with('~') && !raw.starts_with("file://") && Path::new(&written).is_relative())
        .then_some(written)
}

/// `path` is a link resolved against its base. When nothing exists at it, the first root that
/// has the same relative path wins; otherwise `path` comes back unchanged. The relative path is
/// the link as written when it was relative (`written`), else what remains of an absolute
/// `path` below one of the roots, which is how a chat's already bound link looks. A remote
/// `path` is never looked up. `exists` is the disk check.
pub fn reroot(
    path: &str,
    written: Option<&str>,
    roots: &[String],
    exists: impl Fn(&Path) -> bool,
) -> String {
    let bound = Path::new(path);
    if roots.is_empty() || is_remote(bound) || !bound.is_absolute() || exists(bound) {
        return path.to_owned();
    }
    let candidates: Vec<PathBuf> = match written {
        // `..` never leads out of the root it is joined onto: a candidate that does not stay
        // below its root is not a candidate.
        Some(relative) => roots
            .iter()
            .map(|root| {
                (
                    document::resolve_path(Path::new(root), Path::new(root)),
                    document::resolve_path(Path::new(relative), Path::new(root)),
                )
            })
            .filter(|(root, candidate)| candidate.starts_with(root))
            .map(|(_, candidate)| candidate)
            .collect(),
        None => {
            let Some(relative) = roots
                .iter()
                .find_map(|root| bound.strip_prefix(root).ok())
                .filter(|relative| !relative.as_os_str().is_empty())
            else {
                return path.to_owned();
            };
            roots
                .iter()
                .map(|root| PathBuf::from(root).join(relative))
                .collect()
        }
    };
    candidates
        .into_iter()
        .find(|candidate| exists(candidate))
        .map_or_else(
            || path.to_owned(),
            |found| found.to_string_lossy().into_owned(),
        )
}

/// The folders a link that is not where its chat bound it is looked for in, best first: the
/// conversation's project, the session's own worktree, the project's other worktrees
/// (`worktrees_of`), the Mono's other projects, then every project open in the app. Only these
/// folders are joined and checked, nothing is scanned. Empty, remote and repeated folders are
/// dropped: a remote one because looking there would block.
pub fn search_roots(
    project: &str,
    session_worktree: Option<&str>,
    mono_projects: &[String],
    open_projects: &[String],
    worktrees_of: impl Fn(&str) -> Vec<String>,
) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    let mut add = |folder: &str| {
        if !folder.is_empty()
            && folder != "~"
            && !is_remote(Path::new(folder))
            && !roots
                .iter()
                .any(|known| path_key(known) == path_key(folder))
        {
            roots.push(folder.to_owned());
        }
    };
    add(project);
    if let Some(worktree) = session_worktree {
        add(worktree);
    }
    for worktree in worktrees_of(project) {
        add(&worktree);
    }
    for folder in mono_projects.iter().chain(open_projects) {
        add(folder);
    }
    roots
}

/// The git worktrees of `project` a link may be looked for in (checked against the repository,
/// see `git::scoped_worktrees`). Empty for a remote or a non-repository folder.
pub fn project_worktrees(project: &str) -> Vec<String> {
    if project.is_empty() || is_remote(Path::new(project)) {
        return Vec::new();
    }
    crate::git::scoped_worktrees(Path::new(project))
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// The places a missing link was looked for, as one short phrase for an error.
pub fn looked_in(roots: &[String]) -> String {
    const SHOWN: usize = 4;
    let mut places: Vec<&str> = roots.iter().take(SHOWN).map(String::as_str).collect();
    let more = roots.len().saturating_sub(SHOWN);
    let extra = format!("+{more} more");
    if more > 0 {
        places.push(&extra);
    }
    places.join(", ")
}

/// A link's absolute path and its `:line:col` or `#L12` location, resolved against `base` and
/// then re-rooted among `roots`. With no roots, which is every chat that is not a Mono's, this
/// is exactly [`file_location`].
pub fn resolve_link(raw: &str, base: &str, roots: &[String]) -> (String, Option<(usize, usize)>) {
    let (path, location) = file_location(raw, base);
    let written = written_relative(raw);
    (
        reroot(&path, written.as_deref(), roots, |p| p.exists()),
        location,
    )
}

/// Every absolute path a link could mean, computed with no disk access: the link joined onto
/// `base` first, then (for a plain relative link) onto each of `roots`. Remote bases and roots
/// yield nothing beyond the first. Duplicates are dropped.
pub fn link_candidates(raw: &str, base: &str, roots: &[String]) -> Vec<PathBuf> {
    let (path, _) = file_location(raw, base);
    let mut out = vec![PathBuf::from(path)];
    if let Some(written) = written_relative(raw) {
        for root in roots {
            if is_remote(Path::new(root)) {
                continue;
            }
            let candidate = document::resolve_path(Path::new(&written), Path::new(root));
            if !out.iter().any(|known| {
                path_key(&known.to_string_lossy()) == path_key(&candidate.to_string_lossy())
            }) {
                out.push(candidate);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_location_reads_suffixes_and_escapes() {
        assert_eq!(split_location("a.rs:12:3"), ("a.rs".into(), Some((12, 3))));
        assert_eq!(split_location("a.rs:40"), ("a.rs".into(), Some((40, 1))));
        assert_eq!(split_location("a.rs#L7-L9"), ("a.rs".into(), Some((7, 1))));
        assert_eq!(split_location("a.rs#L7C4"), ("a.rs".into(), Some((7, 4))));
        assert_eq!(
            split_location("a.rs#L7C4-L9C2"),
            ("a.rs".into(), Some((7, 4)))
        );
        assert_eq!(split_location("a.rs"), ("a.rs".into(), None));
        assert_eq!(
            split_location("file:///tmp/a%20b.html"),
            ("/tmp/a b.html".into(), None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn candidates_cover_the_base_then_each_root_without_the_disk() {
        let roots = vec!["/p/one".to_owned(), "/p/two".to_owned()];
        let got = link_candidates("src/a.rs:3", "/home/me", &roots);
        assert_eq!(
            got,
            [
                PathBuf::from("/home/me/src/a.rs"),
                PathBuf::from("/p/one/src/a.rs"),
                PathBuf::from("/p/two/src/a.rs"),
            ]
        );
        // An absolute link is never moved under a root.
        assert_eq!(
            link_candidates("/etc/hosts", "/home/me", &roots),
            [PathBuf::from("/etc/hosts")]
        );
        assert_eq!(
            link_candidates("../x.md", "/home/me/a", &[]),
            [PathBuf::from("/home/me/x.md")]
        );
    }
    fn dir(temp: &tempfile::TempDir, name: &str) -> String {
        let path = temp.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn touch(folder: &str, name: &str) -> String {
        let file = Path::new(folder).join(name);
        std::fs::write(&file, "x").unwrap();
        file.to_string_lossy().into_owned()
    }

    fn no_worktrees(_: &str) -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn a_bare_name_is_found_in_the_first_search_root_that_has_it() {
        let temp = tempfile::tempdir().unwrap();
        let (project, tree, other, open) = (
            dir(&temp, "project"),
            dir(&temp, "tree"),
            dir(&temp, "other"),
            dir(&temp, "open"),
        );
        let in_other = touch(&other, "guide.md");
        let in_open = touch(&open, "guide.md");
        let bound = Path::new(&project).join("guide.md");
        let bound = bound.to_string_lossy().into_owned();
        let roots = search_roots(
            &project,
            Some(&tree),
            std::slice::from_ref(&other),
            std::slice::from_ref(&open),
            no_worktrees,
        );
        // Another project of the Mono beats an open one.
        assert_eq!(resolve_link("guide.md", &project, &roots).0, in_other);
        assert_eq!(resolve_link(&bound, &project, &roots).0, in_other);
        // The session's worktree beats both.
        let in_tree = touch(&tree, "guide.md");
        assert_eq!(resolve_link("guide.md", &project, &roots).0, in_tree);
        // Only the open projects are left to answer.
        std::fs::remove_file(&in_tree).unwrap();
        std::fs::remove_file(&in_other).unwrap();
        assert_eq!(resolve_link("guide.md", &project, &roots).0, in_open);
    }

    #[test]
    fn other_worktrees_of_the_project_come_before_other_projects() {
        let temp = tempfile::tempdir().unwrap();
        let (project, tree, other) = (
            dir(&temp, "project"),
            dir(&temp, "tree"),
            dir(&temp, "other"),
        );
        touch(&other, "a.md");
        let in_tree = touch(&tree, "a.md");
        let roots = search_roots(&project, None, std::slice::from_ref(&other), &[], |_| {
            vec![tree.clone()]
        });
        assert_eq!(roots, [project.clone(), tree, other]);
        assert_eq!(resolve_link("a.md", &project, &roots).0, in_tree);
    }

    #[test]
    fn a_file_that_exists_where_it_was_bound_or_an_absolute_link_is_kept() {
        let temp = tempfile::tempdir().unwrap();
        let (project, other) = (dir(&temp, "project"), dir(&temp, "other"));
        let here = touch(&project, "a.md");
        touch(&other, "a.md");
        let roots = search_roots(
            &project,
            None,
            std::slice::from_ref(&other),
            &[],
            no_worktrees,
        );
        assert_eq!(resolve_link("a.md", &project, &roots).0, here);
        // An absolute path outside every root stays where it was written, found or not.
        let missing = temp.path().join("elsewhere").join("a.md");
        let missing = missing.to_string_lossy().into_owned();
        assert_eq!(resolve_link(&missing, &project, &roots).0, missing);
    }

    #[cfg(unix)]
    #[test]
    fn dot_dot_never_leaves_the_root_it_is_joined_onto() {
        let roots = vec!["/p/one".to_owned(), "/p/two".to_owned()];
        // `/p/escape.md` exists, and `../escape.md` from either root would reach it.
        let exists = |path: &Path| path == Path::new("/p/escape.md");
        assert_eq!(
            reroot("/base/missing.md", Some("../escape.md"), &roots, exists),
            "/base/missing.md"
        );
        // A `..` that stays inside the root is fine.
        let exists = |path: &Path| path == Path::new("/p/two/b.md");
        assert_eq!(
            reroot("/base/missing.md", Some("a/../b.md"), &roots, exists),
            "/p/two/b.md"
        );
    }

    #[test]
    fn search_roots_keep_the_order_and_drop_empty_remote_and_repeated_folders() {
        let roots = search_roots(
            "/p/one",
            Some("/p/tree"),
            &["/p/two".to_owned(), "/p/one".to_owned(), String::new()],
            &["/p/three".to_owned(), "/p/two".to_owned(), "~".to_owned()],
            |_| vec!["/p/one".to_owned(), "/p/wt".to_owned()],
        );
        assert_eq!(roots, ["/p/one", "/p/tree", "/p/wt", "/p/two", "/p/three"]);
        assert_eq!(looked_in(&roots), "/p/one, /p/tree, /p/wt, /p/two, +1 more");
        assert_eq!(looked_in(&roots[..2]), "/p/one, /p/tree");
    }
}
