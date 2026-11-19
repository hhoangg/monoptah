//! Which project a relative file link in a Mono chat belongs to, decided when it is clicked.
//!
//! A chat binds a relative link (`crates/x/y.rs`) to its base folder while it draws, with no
//! disk access, and every click path then carries that absolute string: the link menu in
//! `transcript/links.rs` and the shell's `OpenPath` action, which the chat's plain clicks, tool
//! rows, code-block labels and git cards all dispatch. A Mono is not tied to one folder, so the
//! base is only its first project. [`resolve_link`] is the one place that, at click time, looks
//! for the file under each of the Mono's projects (the window's project first) and keeps the
//! bound path when none has it, so the error reads as it always did.
//!
//! A path that exists where the chat bound it is kept: an absolute link and a bound relative
//! one look the same, and an explicit absolute link must open the file it names.
use gpui::{App, Global, Window, WindowId};
use monocode_core::files::{self, remote::is_remote};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// The project each window has open, for the link menu, which runs inside a transcript that
/// cannot see the shell. The shell writes it when it draws.
#[derive(Default)]
struct WindowProjects(HashMap<WindowId, String>);
impl Global for WindowProjects {}

pub(crate) fn note_window_project(window: &Window, project: &str, cx: &mut App) {
    let id = window.window_handle().window_id();
    if cx
        .try_global::<WindowProjects>()
        .is_some_and(|known| known.0.get(&id).is_some_and(|p| p == project))
    {
        return;
    }
    cx.default_global::<WindowProjects>()
        .0
        .insert(id, project.to_owned());
}

pub(crate) fn window_project(window: &Window, cx: &App) -> Option<String> {
    cx.try_global::<WindowProjects>()?
        .0
        .get(&window.window_handle().window_id())
        .cloned()
}

/// The folders a Mono's relative link may belong to, best first: the project the window has
/// open when the Mono works on it, then the Mono's other local projects. A remote project is
/// never a root, because looking for a file there would be a blocking call.
pub(crate) fn candidate_roots(
    window_project: Option<&str>,
    mono_projects: &[String],
) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    let mut add = |project: &str| {
        if !project.is_empty()
            && !is_remote(Path::new(project))
            && !roots
                .iter()
                .any(|known| files::path_key(known) == files::path_key(project))
        {
            roots.push(project.to_owned());
        }
    };
    if let Some(window) = window_project
        && mono_projects
            .iter()
            .any(|p| files::path_key(p) == files::path_key(window))
    {
        add(window);
    }
    for project in mono_projects {
        add(project);
    }
    roots
}

/// `path` is a link resolved against its base. When nothing exists at it, the first root that
/// has the same relative path wins; otherwise `path` comes back unchanged. The relative path is
/// the link as written when it was relative (`written`), else what remains of an absolute
/// `path` below one of the roots, which is how a chat's already bound link looks. A remote
/// `path` is never looked up. `exists` is the disk check.
fn reroot(
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
            .map(|root| files::document::resolve_path(Path::new(relative), Path::new(root)))
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

/// A link's absolute path and its `:line:col` or `#L12` location, resolved against `base`
/// (as [`crate::file_editor::file_location`] does) and then re-rooted among `roots`. With no
/// roots, which is every chat that is not a Mono's, this is exactly `file_location`.
pub(crate) fn resolve_link(
    raw: &str,
    base: &str,
    roots: &[String],
) -> (String, Option<(usize, usize)>) {
    let (path, location) = crate::file_editor::file_location(raw, base);
    let (written, _) = crate::file_editor::split_location(raw);
    // `~/x` and `file://` links are absolute once read; only a plain relative path is "written".
    let relative = (!written.starts_with('~')
        && !raw.starts_with("file://")
        && Path::new(&written).is_relative())
    .then_some(written.as_str());
    (reroot(&path, relative, roots, |p| p.exists()), location)
}

/// The root that holds `path`, for "Copy Relative Path".
pub(crate) fn root_of<'a>(path: &str, roots: &'a [String]) -> Option<&'a str> {
    roots
        .iter()
        .find(|root| Path::new(path).starts_with(root))
        .map(String::as_str)
}

#[cfg(test)]
mod tests;
