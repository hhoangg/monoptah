//! The per-session cache of cloud files that did not fit the in-memory preview.
//!
//! Files land in `<session folder>/cloud-cache/` keeping their workspace-relative path.
//! Every download is streamed through the Host's `files.readBytes` (see [`super::view`]) into
//! `<name>.cloud-part`, checked against the size the Host reported, and only then renamed
//! into place, so a cancelled or broken download never leaves a half file under its real name.
//!
//! Safety rules: a cache path is built from validated segments only (no `..`, no empty or
//! hidden-traversal parts, no separators), every directory below the cache root is checked
//! with `lstat` and must be a real directory (a symlink there refuses the download), and the
//! file is renamed over whatever was there, never written through a link.
//!
//! Hygiene: [`evict`] keeps all sessions' caches together under [`CACHE_CAP_BYTES`] by
//! deleting the oldest files first; the session folder's removal (delete) and
//! [`CloudSessions::clear_cloud_cache`] (archive) delete the whole cache.

use super::view::{
    css_references, html_references, kind_of, Chunk, CloudFile, Fetcher, FileKind, ViewError,
};
use super::CloudSessions;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::LazyLock;

/// A file above this size is only downloaded after the user confirms.
pub const CONFIRM_BYTES: u64 = 500 * 1024 * 1024;
/// Total size of all sessions' caches before the oldest files are deleted.
pub const CACHE_CAP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Most assets fetched next to one downloaded HTML page.
const MAX_BUNDLE_FILES: usize = 200;
/// Prefix of every temporary file or folder of an unfinished download. Their names carry the
/// process id and a counter, so they never collide with a real file; [`evict`] skips them.
const TEMP_PREFIX: &str = ".monoptah-";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_name(kind: &str) -> String {
    format!(
        "{TEMP_PREFIX}{kind}-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Largest HTML page that gets the content security policy written into its cached copy.
const MAX_PAGE_BYTES: u64 = 50 * 1024 * 1024;

/// The policy a page loaded from the cache runs under: no network, no frames, no forms,
/// only the cached files and inline code.
pub const PAGE_CSP: &str = "default-src 'none'; script-src 'unsafe-inline' file:; style-src 'unsafe-inline' file:; img-src file: data:; font-src file: data:; media-src file: data:; connect-src 'none'; form-action 'none'";

static HEAD_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)<head(\s[^>]*)?>").expect("static"));
static DOCTYPE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?is)<!doctype[^>]*>").expect("static"));

/// `html` with a Content-Security-Policy meta tag as early as possible: right after
/// `<head>`, else after the doctype, else first.
pub fn with_csp(html: &str) -> String {
    let meta = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{PAGE_CSP}\">");
    let at = HEAD_RE
        .find(html)
        .or_else(|| DOCTYPE_RE.find(html))
        .map_or(0, |found| found.end());
    format!("{}{meta}{}", &html[..at], &html[at..])
}

/// Marks a downloaded file as coming from the internet, so macOS asks before it is run.
fn quarantine(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut command = std::process::Command::new("xattr");
        command
            .args(["-w", "com.apple.quarantine"])
            .arg(format!("0083;{stamp:x};Monoptah;"))
            .arg(path);
        crate::process::configure_child(&mut command);
        let _ = command.output();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

/// Rewrites the cached page `path` with the policy in it.
fn write_csp(path: &Path) -> Result<(), ViewError> {
    let fail = |e: std::io::Error| ViewError::Failed(e.to_string());
    let meta = std::fs::metadata(path).map_err(fail)?;
    if meta.len() > MAX_PAGE_BYTES {
        return Err(ViewError::Failed(
            "This page is too large to show safely.".into(),
        ));
    }
    let html = String::from_utf8_lossy(&std::fs::read(path).map_err(fail)?).into_owned();
    let temp = path.with_file_name(temp_name("page"));
    std::fs::write(&temp, with_csp(&html)).map_err(fail)?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        fail(e)
    })
}

/// A file name safe to put in a message.
pub(crate) fn sanitize_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// The cache location of workspace-relative `rel` under `cache_dir`. Pure: no file access.
pub fn cache_path(cache_dir: &Path, rel: &str) -> Result<PathBuf, ViewError> {
    let bad = || ViewError::Refused(format!("{} can not be cached.", sanitize_name(rel)));
    let mut path = cache_dir.to_path_buf();
    let mut any = false;
    for part in rel.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > 200
            || part.eq_ignore_ascii_case(".git")
            || part.contains(['\0', '\\', ':'])
            || part.chars().any(char::is_control)
        {
            return Err(bad());
        }
        path.push(part);
        any = true;
    }
    if !any {
        return Err(bad());
    }
    Ok(path)
}

/// Creates the folders above `target` below `cache_dir`, refusing links and files on the way.
fn prepare_parent(cache_dir: &Path, target: &Path) -> Result<(), ViewError> {
    let fail = |text: String| ViewError::Failed(text);
    std::fs::create_dir_all(cache_dir).map_err(|e| fail(e.to_string()))?;
    let unsafe_path = || fail("The cache folder contains a link and was not used.".into());
    let meta = std::fs::symlink_metadata(cache_dir).map_err(|e| fail(e.to_string()))?;
    if !meta.file_type().is_dir() {
        return Err(unsafe_path());
    }
    let below = target
        .parent()
        .and_then(|parent| parent.strip_prefix(cache_dir).ok())
        .ok_or_else(unsafe_path)?;
    let mut current = cache_dir.to_path_buf();
    for part in below.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return Err(unsafe_path()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|e| fail(e.to_string()))?;
            }
            Err(e) => return Err(fail(e.to_string())),
        }
    }
    Ok(())
}

/// Deletes the oldest files of `dirs` (every cache folder) until they total at most `cap`
/// bytes. Files in `keep` and links are never touched. Returns the bytes removed.
pub fn evict(dirs: &[PathBuf], cap: u64, keep: &[PathBuf]) -> u64 {
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    fn walk(dir: &Path, files: &mut Vec<(std::time::SystemTime, u64, PathBuf)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // An unfinished download is not evictable.
            if entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
                continue;
            }
            if kind.is_dir() {
                walk(&entry.path(), files);
            } else if kind.is_file() {
                if let Ok(meta) = entry.metadata() {
                    let at = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    files.push((at, meta.len(), entry.path()));
                }
            }
        }
    }
    for dir in dirs {
        walk(dir, &mut files);
    }
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort();
    let mut removed = 0;
    for (_, len, path) in files {
        if total <= cap {
            break;
        }
        if keep.contains(&path) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
            removed += len;
            // Tidy empty folders up to (not including) the cache root.
            let mut parent = path.parent();
            while let Some(dir) = parent {
                if dirs.iter().any(|root| root == dir) || std::fs::remove_dir(dir).is_err() {
                    break;
                }
                parent = dir.parent();
            }
        }
    }
    removed
}

/// One downloaded file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloaded {
    pub kind: FileKind,
    /// The cached copy of the requested file.
    pub path: PathBuf,
    /// The session's `cloud-cache` folder: the read-access root for a web view.
    pub cache_dir: PathBuf,
    pub size: u64,
}

/// Streams `rel` into the cache. `base` is the byte count already done (for the progress
/// callback, which gets `(done, total)`), `total` the expected total of the whole download.
fn download_one(
    fetcher: &Fetcher,
    rel: &str,
    cache_dir: &Path,
    first: Chunk,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<(PathBuf, u64), ViewError> {
    let target = cache_path(cache_dir, rel)?;
    prepare_parent(cache_dir, &target)?;
    let part = target.with_file_name(temp_name("part"));
    let fail = |e: std::io::Error| ViewError::Failed(e.to_string());
    let expected = first.size;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(fail)?;
    let mut written = 0u64;
    let result = fetcher.stream(rel, first, cancel, &mut |bytes| {
        file.write_all(bytes).map_err(fail)?;
        written += bytes.len() as u64;
        progress(bytes.len() as u64);
        Ok(())
    });
    let finished = result.and_then(|count| {
        file.flush().map_err(fail)?;
        drop(file);
        let on_disk = std::fs::metadata(&part).map_err(fail)?.len();
        if count != expected || written != expected || on_disk != expected {
            return Err(ViewError::Failed(format!(
                "The download was incomplete ({on_disk} of {expected} bytes). Try again."
            )));
        }
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_dir()) {
            return Err(ViewError::Failed(
                "A folder is in the way of the download.".into(),
            ));
        }
        std::fs::rename(&part, &target).map_err(fail)
    });
    if let Err(error) = finished {
        let _ = std::fs::remove_file(&part);
        return Err(error);
    }
    Ok((target, expected))
}

impl CloudSessions {
    /// Downloads `file` into the session's cloud cache (HTML pages also get the assets they
    /// reference, so they render from disk). `progress` gets `(done, total)` bytes. A file
    /// over [`CONFIRM_BYTES`] needs `confirmed`; otherwise [`ViewError::NeedsConfirm`].
    pub fn download_cloud_file(
        &self,
        file: &CloudFile,
        confirmed: bool,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Downloaded, ViewError> {
        let cache_dir = file.cache_dir.clone().ok_or_else(|| {
            ViewError::Failed(
                "This store keeps no session folders, so nothing can be saved.".into(),
            )
        })?;
        let fetcher = Fetcher::new(&*self.host, file);
        let first = fetcher.first(&file.rel)?;
        let size = first.size;
        if size > CONFIRM_BYTES && !confirmed {
            return Err(ViewError::NeedsConfirm { size });
        }
        let mut done = 0u64;
        let mut total = size;
        progress(0, total);
        let (path, _) = download_one(&fetcher, &file.rel, &cache_dir, first, cancel, &mut |n| {
            done += n;
            progress(done, total);
        })?;
        let mut keep = vec![path.clone()];
        if file.kind() == FileKind::Html && size <= super::view::MAX_INLINE_BYTES {
            self.download_assets(
                &fetcher, file, &cache_dir, &path, cancel, &mut done, &mut total, &mut keep,
                progress,
            )?;
        }
        if file.kind() == FileKind::Html {
            write_csp(&path)?;
        }
        quarantine(&path);
        // Best effort: a failed cleanup must not fail a finished download.
        let dirs = self
            .store()
            .map(|store| store.cloud_cache_dirs())
            .unwrap_or_default();
        evict(&dirs, CACHE_CAP_BYTES, &keep);
        Ok(Downloaded {
            kind: file.kind(),
            path,
            cache_dir,
            size,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn download_assets(
        &self,
        fetcher: &Fetcher,
        file: &CloudFile,
        cache_dir: &Path,
        page: &Path,
        cancel: &AtomicBool,
        done: &mut u64,
        total: &mut u64,
        keep: &mut Vec<PathBuf>,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<(), ViewError> {
        let html = std::fs::read(page)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|e| ViewError::Failed(e.to_string()))?;
        let base = |rel: &str| {
            rel.rsplit_once('/')
                .map_or(String::new(), |(d, _)| d.to_owned())
        };
        let mut queue: Vec<String> = html_references(&file.root, &base(&file.rel), &html);
        let mut seen: HashSet<String> = queue.iter().cloned().collect();
        seen.insert(file.rel.clone());
        let mut spent = 0u64;
        let mut index = 0;
        while index < queue.len() && index < MAX_BUNDLE_FILES {
            let rel = queue[index].clone();
            index += 1;
            // Missing, refused or oversized assets just stay broken in the page.
            let first = match fetcher.first(&rel) {
                Ok(first) => first,
                Err(error @ (ViewError::NotConnected(_) | ViewError::Cancelled)) => {
                    return Err(error)
                }
                Err(_) => continue,
            };
            if spent + first.size > CONFIRM_BYTES {
                continue;
            }
            spent += first.size;
            *total += first.size;
            let size = first.size;
            let fetched = download_one(fetcher, &rel, cache_dir, first, cancel, &mut |n| {
                *done += n;
                progress(*done, *total);
            });
            match fetched {
                Ok((path, _)) => {
                    keep.push(path.clone());
                    let is_css = kind_of(&rel) == FileKind::Text
                        && rel.to_ascii_lowercase().ends_with(".css")
                        && size <= super::view::MAX_MEMORY_BYTES;
                    if let (true, Ok(bytes)) = (is_css, std::fs::read(&path)) {
                        let css = String::from_utf8_lossy(&bytes).into_owned();
                        for reference in css_references(&css) {
                            let found =
                                super::view::reference_target(&file.root, &base(&rel), &reference);
                            if let Some(found) = found {
                                if seen.insert(found.clone()) {
                                    queue.push(found);
                                }
                            }
                        }
                    }
                }
                Err(error @ (ViewError::NotConnected(_) | ViewError::Cancelled)) => {
                    return Err(error)
                }
                Err(_) => {}
            }
        }
        Ok(())
    }

    /// Deletes the cloud cache of session `session_id` (on archive; deleting the session
    /// removes its whole folder anyway).
    pub fn clear_cloud_cache(&self, session_id: i64) -> Result<(), String> {
        self.store()?.clear_cloud_cache(session_id)
    }
}
