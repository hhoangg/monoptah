//! Pure resolution of a file link as written in a chat: `~`, `file://`, `%` escapes, a `:12:3` or
//! `#L12` location, a join onto the chat's folder, and the re-rooting among a Mono's project
//! folders. It lives here so the app's click handling (`file_links.rs` in the GPUI crate) and the
//! remote `chat.mentionedFile` method decide the same path for the same text.
use std::path::{Path, PathBuf};

use super::{document, path_key, remote::is_remote};

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
fn written_relative(raw: &str) -> Option<String> {
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
        Some(relative) => roots
            .iter()
            .map(|root| document::resolve_path(Path::new(relative), Path::new(root)))
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
}
