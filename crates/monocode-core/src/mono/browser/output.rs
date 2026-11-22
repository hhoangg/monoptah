//! The folder Playwright writes into (`--output-dir`): page snapshots (`page-*.yml`), find
//! results, screenshots, console logs and the files the Mono asks for with `filename`. They hold
//! page content from the user's logged-in Chrome, so they are sensitive: the folder is private to
//! the user (mode 0700), it is the server's working directory too (nothing lands in the app's own
//! working directory or in `~/.playwright-mcp`), a `filename` the Mono gives is forced into it,
//! and old files are removed.
//!
//! Recipes are kept elsewhere (see `recipe::store`), so a sweep here can never touch them.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Files older than this are removed by [`sweep`].
pub const MAX_AGE: Duration = Duration::from_secs(2 * 60 * 60);
/// At most this many files are kept by [`sweep`] (the newest).
pub const MAX_FILES: usize = 60;
/// Tools of `@playwright/mcp` that take a `filename` to save their output.
pub const FILE_TOOLS: [&str; 6] = [
    "browser_snapshot",
    "browser_find",
    "browser_take_screenshot",
    "browser_console_messages",
    "browser_network_requests",
    "browser_network_request",
];

/// Creates the folder if needed and keeps it private to the user.
pub fn prepare(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Removes the regular files under `dir` (subfolders included) that are older than `max_age`,
/// then the oldest ones beyond `max_files`, then subfolders left empty. Symlinks are never
/// followed or removed through. Returns how many files went.
pub fn sweep_with(dir: &Path, now: SystemTime, max_age: Duration, max_files: usize) -> usize {
    fn collect(
        dir: &Path,
        files: &mut Vec<(SystemTime, PathBuf)>,
        folders: &mut Vec<PathBuf>,
        now: SystemTime,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                folders.push(path.clone());
                collect(&path, files, folders, now);
            } else if meta.is_file() {
                files.push((meta.modified().unwrap_or(now), path));
            }
        }
    }
    let mut files = Vec::new();
    let mut folders = Vec::new();
    collect(dir, &mut files, &mut folders, now);
    files.sort();
    let mut removed = 0;
    let total = files.len();
    for (index, (modified, path)) in files.iter().enumerate() {
        let too_old = now.duration_since(*modified).is_ok_and(|age| age > max_age);
        let too_many = total - index > max_files;
        if (too_old || too_many) && std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    // Deepest first; `remove_dir` only succeeds on an empty folder.
    folders.sort_by_key(|f| std::cmp::Reverse(f.components().count()));
    for folder in folders {
        let _ = std::fs::remove_dir(folder);
    }
    removed
}
pub fn sweep(dir: &Path) -> usize {
    sweep_with(dir, SystemTime::now(), MAX_AGE, MAX_FILES)
}

/// Removes every regular file of `dir` (the browser was turned off or the Mono was removed).
pub fn clear(dir: &Path) -> usize {
    sweep_with(dir, SystemTime::now(), Duration::ZERO, 0)
}

/// A plain file name from whatever the Mono asked for: only the last path component, with
/// anything but letters, digits, `.`, `-` and `_` replaced, never starting with a dot.
pub fn safe_file_name(requested: &str) -> Option<String> {
    let last = requested
        .trim()
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())?;
    let cleaned: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let cleaned = cleaned.trim_start_matches('.').to_owned();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Forces the `filename` argument of a file-writing tool into `dir`: Playwright resolves a plain
/// name against its workspace, and a path outside the output folder would put page content
/// somewhere the app does not manage. A call without `filename` is left as it is. A name that
/// already exists as a symlink is refused (writing through it would leave the folder).
pub fn confine_filename(tool: &str, args: &mut Value, dir: &Path) -> Result<(), String> {
    if !FILE_TOOLS.contains(&tool) {
        return Ok(());
    }
    let Some(map) = args.as_object_mut() else {
        return Ok(());
    };
    let Some(requested) = map.get("filename").and_then(Value::as_str) else {
        return Ok(());
    };
    match safe_file_name(requested) {
        Some(name) => {
            let path = dir.join(name);
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("That file name is a symbolic link; use another name.".into());
            }
            map.insert(
                "filename".into(),
                Value::String(path.to_string_lossy().into_owned()),
            );
        }
        None => {
            map.remove("filename");
        }
    }
    Ok(())
}

/// The saved file a tool result links to (`- [Snapshot](page-123.yml)`), as a path inside `dir`.
/// Only the file name is used, so a link can never point the reader outside the folder.
pub fn linked_file(text: &str, dir: &Path) -> Option<PathBuf> {
    let start = text.find("](")? + 2;
    let link = text[start..].split(')').next()?;
    if !(link.ends_with(".yml") || link.ends_with(".md")) {
        return None;
    }
    let path = dir.join(safe_file_name(link)?);
    path.is_file().then_some(path)
}
