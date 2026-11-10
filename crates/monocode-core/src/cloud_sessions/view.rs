//! Viewing a file of a cloud workspace (a codespace or a Docker sandbox) from the chat.
//!
//! The agent mentions paths such as `/workspaces/repo/.evidence/report.html`; a click must
//! show that file in the app even though it only exists in the cloud. Everything here goes
//! through the Host's file calls over the session's live tunnel, so it never wakes or
//! resumes anything: without a tunnel the answer is [`ViewError::NotConnected`] ("Reconnect
//! to view ..."). A click on a link is a user action, so the app then reconnects the
//! workspace through its own user-initiated door (the same one as the Reconnect button, see
//! `docker_start::reconnect` for a sandbox) and reads again; this module never does.
//! The ssh based `CloudBackend::copy_from` is deliberately not used.
//!
//! # Host calls
//!
//! * `files.readBytes` `{projectId, path, offset, length}` returns
//!   `{data: base64, offset: next, size}`. At most `MAX_READ_CHUNK` (393216) bytes per call,
//!   any file type, any offset; the path is checked by the Host (inside the project, no
//!   `.git` segment, symlinks must not leave the workspace).
//! * `files.read` is the older text only call (UTF-8, at most 1 MB, binary refused). A Host
//!   that predates `files.readBytes` answers "Unsupported host method"; small text files and
//!   Markdown/HTML still open through `files.read`, anything else says the Host is out of
//!   date.
//!
//! # Flow (all BLOCKING; call from a worker thread)
//!
//! 1. [`CloudSessions::resolve_cloud_file`] `(session, raw path) -> CloudFile`: a store and
//!    machine list read. Paths are resolved against the session's remote cwd and must stay
//!    inside it ([`resolve_workspace_path`]); anything else is [`ViewError::Refused`].
//! 2. [`CloudSessions::open_cloud_file`] `(&CloudFile) -> Opened`: the first read reveals the
//!    size. A previewable file up to [`MAX_MEMORY_BYTES`] is returned as bytes, text or (HTML,
//!    with relative assets inlined as `data:` URIs up to [`MAX_INLINE_BYTES`]) a single page,
//!    never touching disk. Anything else is a [`DownloadPlan`].
//! 3. [`CloudSessions::download_cloud_file`] downloads into the session's
//!    `cloud-cache/` (see [`super::view_cache`]).

use super::backend::CloudRef;
use super::view_cache::sanitize_name;
use super::CloudSessions;
use crate::remote::Machine;
use crate::store::is_pending_cloud_environment;
use base64::Engine;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

fn pattern(source: &str) -> regex::Regex {
    regex::Regex::new(source).expect("static pattern")
}
static URL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| pattern(r#"(?is)url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s"']*))\s*\)"#));
static STYLE_BLOCK_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| pattern(r"(?is)(<style\b[^>]*>)(.*?)(</style\s*>)"));
static STYLE_ATTR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| pattern(r#"(?is)(\sstyle\s*=\s*)("([^"]*)"|'([^']*)')"#));
static TAG_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| pattern(r"(?is)<(img|source|video|audio|link|script)\b([^>]*)>"));
/// One pattern per attribute name the page rewriting looks at.
static ATTRIBUTE_RES: LazyLock<Vec<(&'static str, regex::Regex)>> = LazyLock::new(|| {
    ["src", "href", "poster", "rel"]
        .into_iter()
        .map(|name| {
            (
                name,
                pattern(&format!(
                    r#"(?is)(?:^|\s){name}\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>"']+))"#
                )),
            )
        })
        .collect()
});

/// Largest previewable file read into memory.
pub const MAX_MEMORY_BYTES: u64 = 5 * 1024 * 1024;
/// Largest total of the assets inlined into one HTML page.
pub const MAX_INLINE_BYTES: u64 = 20 * 1024 * 1024;
/// Chunk size asked of the Host (divisible by three, equal to its own cap).
pub const CHUNK_BYTES: u64 = 393_216;

/// What the viewers can do with a file, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// png, jpg, gif, webp: what the image viewer decodes.
    Image,
    Markdown,
    Html,
    /// Source code and plain text.
    Text,
    /// Anything else: only "Open with default app" and "Reveal".
    Other,
}

impl FileKind {
    /// Kinds the app can show itself.
    pub fn previewable(self) -> bool {
        self != FileKind::Other
    }
}

/// Classifies a file name.
pub fn kind_of(name: &str) -> FileKind {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => FileKind::Image,
        "md" | "markdown" => FileKind::Markdown,
        "html" | "htm" => FileKind::Html,
        "txt" | "log" | "json" | "jsonl" | "yaml" | "yml" | "toml" | "csv" | "tsv" | "xml"
        | "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py" | "rb" | "go" | "java"
        | "kt" | "swift" | "c" | "h" | "cc" | "cpp" | "hpp" | "cs" | "php" | "sh" | "bash"
        | "zsh" | "fish" | "css" | "scss" | "less" | "sql" | "diff" | "patch" | "ini" | "cfg"
        | "conf" | "lock" | "gradle" | "tf" | "proto" | "vue" | "svelte" | "lua" | "r" => {
            FileKind::Text
        }
        "" => match lower.as_str() {
            "makefile" | "dockerfile" | "readme" | "license" | "changelog" | "gemfile"
            | "procfile" => FileKind::Text,
            _ => FileKind::Other,
        },
        _ => FileKind::Other,
    }
}

/// A file name safe to put in a message.
pub(crate) fn name_for_message(name: &str) -> String {
    super::view_cache::sanitize_name(name)
}

/// Files a double click would run or install (apps, scripts, installers, shortcuts).
/// They are only revealed, never opened with the default app.
pub fn runs_code(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    matches!(
        ext,
        "app"
            | "command"
            | "terminal"
            | "webloc"
            | "tool"
            | "sh"
            | "bash"
            | "zsh"
            | "pkg"
            | "dmg"
            | "exe"
            | "msi"
            | "bat"
            | "cmd"
            | "ps1"
            | "jar"
            | "scpt"
            | "workflow"
            | "action"
            | "service"
            | "desktop"
            | "appimage"
            | "run"
            | "inetloc"
            | "url"
    )
}

/// MIME type of an image the viewer can decode.
pub fn image_mime(name: &str) -> Option<&'static str> {
    match name.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Why a file could not be shown. `Display` is the user-facing text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// No live tunnel: the text says to reconnect. Nothing was woken.
    NotConnected(String),
    /// The path is not something to fetch (outside the workspace, not a cloud session).
    Refused(String),
    Cancelled,
    /// The Host lacks `files.readBytes`: only the user-started copy can fetch this file.
    OldHost,
    /// The file is over the confirm threshold; ask, then retry with `confirmed`.
    NeedsConfirm {
        size: u64,
    },
    Failed(String),
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::NotConnected(text) | ViewError::Refused(text) | ViewError::Failed(text) => {
                f.write_str(text)
            }
            ViewError::Cancelled => f.write_str("Cancelled."),
            ViewError::OldHost => f.write_str(OUT_OF_DATE),
            ViewError::NeedsConfirm { size } => {
                write!(f, "This file is {} MB.", size / (1024 * 1024))
            }
        }
    }
}

/// A file of a cloud workspace, resolved for one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudFile {
    pub session_id: i64,
    pub cloud: CloudRef,
    /// The Connections machine of the session's workspace.
    pub machine_id: String,
    pub project_id: String,
    /// The session's Host worktree, sent as `cwd` in file reads; `None` when it runs in the
    /// project folder (or its worktree was removed).
    pub cwd: Option<String>,
    /// The session's remote cwd (its worktree when it has one), the only root files are
    /// served from.
    pub root: String,
    /// Path below `root`, `/` separated, every segment validated.
    pub rel: String,
    /// `<session folder>/cloud-cache`, `None` for a store without session folders.
    pub cache_dir: Option<PathBuf>,
}

impl CloudFile {
    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }
    pub fn kind(&self) -> FileKind {
        kind_of(self.name())
    }
    /// The absolute path in the workspace, for messages.
    pub fn remote_path(&self) -> String {
        format!("{}/{}", self.root.trim_end_matches('/'), self.rel)
    }
}

/// Why a file goes to the download path instead of memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanReason {
    TooLarge,
    NotPreviewable,
    /// An HTML page whose assets do not fit in memory.
    HtmlAssets,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadPlan {
    pub size: u64,
    pub kind: FileKind,
    pub reason: PlanReason,
    /// Over the per-file threshold ([`super::view_cache::CONFIRM_BYTES`]): ask first.
    pub needs_confirm: bool,
}

/// What opening a file produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    Image {
        bytes: Vec<u8>,
        mime: &'static str,
    },
    /// Markdown or code/text (`kind` says which).
    Text {
        kind: FileKind,
        text: String,
    },
    /// One self-contained page.
    Html {
        html: String,
    },
    Download(DownloadPlan),
    /// The Host cannot send this file (it lacks `files.readBytes`); the user may start a copy
    /// with [`CloudSessions::copy_cloud_file`].
    CopyOnly {
        kind: FileKind,
    },
}

/// Resolves `raw` (absolute, or relative to `root`) to a validated path below `root`.
///
/// Dot segments are collapsed lexically; symlinks are never consulted here (the Host
/// refuses links leaving the workspace). Returns the `/` separated relative path.
pub fn resolve_workspace_path(raw: &str, root: &str) -> Result<String, ViewError> {
    let shown = raw.trim();
    let outside = || {
        ViewError::Refused(format!(
            "{shown} is outside the cloud workspace ({root}); only files inside it can be viewed."
        ))
    };
    if shown.is_empty() || shown.contains('\0') || shown.contains('\\') {
        return Err(ViewError::Refused(format!(
            "{shown:?} is not a valid file path."
        )));
    }
    let normalise = |path: &str| -> Option<Vec<String>> {
        let mut parts: Vec<String> = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                other => parts.push(other.to_owned()),
            }
        }
        Some(parts)
    };
    let root_parts = normalise(root).ok_or_else(outside)?;
    let joined = if shown.starts_with('/') {
        shown.to_owned()
    } else {
        format!("{}/{shown}", root.trim_end_matches('/'))
    };
    let parts = normalise(&joined).ok_or_else(outside)?;
    if parts.len() <= root_parts.len() || parts[..root_parts.len()] != root_parts[..] {
        return Err(outside());
    }
    let rel = &parts[root_parts.len()..];
    for part in rel {
        if part.len() > 255 || part.chars().any(char::is_control) {
            return Err(ViewError::Refused(format!(
                "{shown} is not a valid file path."
            )));
        }
        if part.eq_ignore_ascii_case(".git") {
            return Err(ViewError::Refused(
                "Files inside .git are not available from the cloud workspace.".into(),
            ));
        }
    }
    Ok(rel.join("/"))
}

/// One range of a file as the Host returned it.
pub(crate) struct Chunk {
    pub(crate) data: Vec<u8>,
    pub(crate) size: u64,
}

/// Reads files of one [`CloudFile`]'s workspace through the Host.
pub(crate) struct Fetcher<'a> {
    host: &'a dyn super::Host,
    file: &'a CloudFile,
    /// Whether the Host advertises `files.readBytes`, asked once (`None`: not asked yet).
    /// Without it only `files.read` serves small text files.
    bytes: Cell<Option<bool>>,
}

impl<'a> Fetcher<'a> {
    pub(crate) fn new(host: &'a dyn super::Host, file: &'a CloudFile) -> Self {
        Self {
            host,
            file,
            bytes: Cell::new(None),
        }
    }

    fn map_error(&self, error: String) -> ViewError {
        if super::is_not_connected(&error) || error.starts_with("Machine is no longer connected") {
            ViewError::NotConnected(format!("Reconnect to view {}", self.file.name()))
        } else {
            ViewError::Failed(error)
        }
    }

    /// Whether the Host advertises `files.readBytes` in its capabilities (from the cached
    /// [`HostInfo`](crate::remote::host_info::HostInfo); with no live tunnel it is the
    /// not-connected answer).
    pub(crate) fn has_bytes(&self) -> Result<bool, ViewError> {
        if let Some(known) = self.bytes.get() {
            return Ok(known);
        }
        let known = self
            .host
            .host_info(&self.file.machine_id)
            .map_err(|e| self.map_error(e))?
            .supports("files.readBytes");
        self.bytes.set(Some(known));
        Ok(known)
    }

    /// Reads up to `length` bytes of `rel` at `offset`.
    fn range(&self, rel: &str, offset: u64, length: u64) -> Result<Chunk, ViewError> {
        if self.has_bytes()? {
            let mut params = json!({
                "projectId": self.file.project_id,
                "path": rel,
                "offset": offset,
                "length": length.min(CHUNK_BYTES),
            });
            self.add_cwd(&mut params);
            return match self
                .host
                .request(&self.file.machine_id, "files.readBytes", params)
            {
                Ok(value) => self.decode(&value, offset),
                Err(error) => Err(self.map_error(error)),
            };
        }
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if offset != 0
            || !matches!(
                kind_of(name),
                FileKind::Markdown | FileKind::Html | FileKind::Text
            )
        {
            return Err(ViewError::OldHost);
        }
        let mut params = json!({"projectId": self.file.project_id, "path": rel});
        self.add_cwd(&mut params);
        match self
            .host
            .request(&self.file.machine_id, "files.read", params)
        {
            Ok(Value::String(text)) => Ok(Chunk {
                size: text.len() as u64,
                data: text.into_bytes(),
            }),
            Ok(_) => Err(ViewError::Failed(
                "The host sent an unexpected reply.".into(),
            )),
            Err(error) if super::is_not_connected(&error) => Err(self.map_error(error)),
            Err(_) => Err(ViewError::OldHost),
        }
    }

    /// Adds the session's worktree `cwd` to file read `params`, when it has one.
    fn add_cwd(&self, params: &mut Value) {
        if let Some(cwd) = &self.file.cwd {
            params["cwd"] = json!(cwd);
        }
    }

    fn decode(&self, value: &Value, offset: u64) -> Result<Chunk, ViewError> {
        let bad = || ViewError::Failed("The host sent an unexpected range.".into());
        let data = base64::engine::general_purpose::STANDARD
            .decode(value["data"].as_str().ok_or_else(bad)?)
            .map_err(|_| bad())?;
        let size = value["size"].as_u64().ok_or_else(bad)?;
        if value["offset"].as_u64() != Some(offset + data.len() as u64)
            || data.len() as u64 > CHUNK_BYTES
        {
            return Err(bad());
        }
        Ok(Chunk { data, size })
    }

    /// Streams the whole file `rel` into `sink`, starting from its already read `first`
    /// chunk. Returns the byte count, which equals the size the Host reported.
    pub(crate) fn stream(
        &self,
        rel: &str,
        first: Chunk,
        cancel: &AtomicBool,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), ViewError>,
    ) -> Result<u64, ViewError> {
        let size = first.size;
        let mut offset = 0u64;
        let mut chunk = first;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(ViewError::Cancelled);
            }
            if chunk.size != size || (chunk.data.is_empty() && offset < size) {
                return Err(ViewError::Failed(
                    "The file changed while it was being read. Try again.".into(),
                ));
            }
            sink(&chunk.data)?;
            offset += chunk.data.len() as u64;
            if offset >= size {
                return Ok(offset);
            }
            chunk = self.range(rel, offset, CHUNK_BYTES)?;
        }
    }

    pub(crate) fn first(&self, rel: &str) -> Result<Chunk, ViewError> {
        self.range(rel, 0, CHUNK_BYTES)
    }

    /// The whole file `rel` in memory; `Ok(Err(size))` when it is over `limit`.
    fn read_all(
        &self,
        rel: &str,
        limit: u64,
        cancel: &AtomicBool,
    ) -> Result<Result<Vec<u8>, u64>, ViewError> {
        let first = self.first(rel)?;
        if first.size > limit {
            return Ok(Err(first.size));
        }
        let mut out = Vec::with_capacity(first.size as usize);
        self.stream(rel, first, cancel, &mut |bytes| {
            out.extend_from_slice(bytes);
            Ok(())
        })?;
        Ok(Ok(out))
    }
}

/// Shown when the Host predates `files.readBytes`.
pub const OUT_OF_DATE: &str = "The Monoptah Host in this workspace is too old to send this file. It updates itself the next time this workspace connects, or you can download a copy with the button below.";

impl CloudSessions {
    /// Resolves a path mentioned in session `session_id` into a [`CloudFile`]. Reads the
    /// store and the machine list; sends nothing to the workspace.
    pub fn resolve_cloud_file(&self, session_id: i64, raw: &str) -> Result<CloudFile, ViewError> {
        let (session, cache_dir, settings) = {
            let store = self.store().map_err(ViewError::Failed)?;
            let session = store
                .remote_session(session_id)
                .map_err(ViewError::Failed)?
                .ok_or_else(|| ViewError::Failed("Session not found".into()))?;
            let cache_dir = store
                .cloud_cache_dir(session_id)
                .map_err(ViewError::Failed)?;
            let settings = store
                .agent_settings(session_id)
                .map_err(ViewError::Failed)?;
            (session, cache_dir, settings)
        };
        let cloud = session.project.cloud().ok_or_else(|| {
            ViewError::Refused("This session does not run in a cloud workspace.".into())
        })?;
        if is_pending_cloud_environment(&session.project.environment_id) {
            return Err(ViewError::Failed(super::startup::STILL_STARTING.into()));
        }
        // A session in a Host worktree reads from that worktree, not the project root.
        let cwd = settings
            .as_ref()
            .and_then(super::stored_worktree_cwd)
            .and_then(|cwd| super::confine_host_cwd(&cwd, &self.workspace_root_of(&cloud)));
        let root = cwd.clone().unwrap_or_else(|| session.project.cwd.clone());
        // A `remote://<environment>/path` link must name this session's own machine.
        let raw = match crate::remote::parse_remote_path(raw.trim()) {
            Some(remote) if remote.environment_id == session.project.environment_id => {
                remote.host_path
            }
            Some(_) => {
                return Err(ViewError::Refused(
                    "This link points to another machine, not this cloud workspace.".into(),
                ))
            }
            None => raw.to_owned(),
        };
        let rel = resolve_workspace_path(&raw, &root)?;
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_owned();
        let machine: Option<Machine> = self
            .host
            .machines()
            .map_err(ViewError::Failed)?
            .into_iter()
            .find(|m| m.environment_id == session.project.environment_id);
        let Some(machine) = machine else {
            return Err(ViewError::NotConnected(format!(
                "Reconnect to view {}",
                sanitize_name(&name)
            )));
        };
        Ok(CloudFile {
            session_id,
            cloud,
            machine_id: machine.id,
            project_id: session.project.project_id.clone(),
            cwd,
            root,
            rel,
            cache_dir,
        })
    }

    /// Reads `file` into memory when it is previewable and small enough; otherwise says how
    /// it must be downloaded. One Host request when the answer is a download plan.
    pub fn open_cloud_file(
        &self,
        file: &CloudFile,
        cancel: &AtomicBool,
    ) -> Result<Opened, ViewError> {
        let fetcher = Fetcher::new(&*self.host, file);
        let kind = file.kind();
        let first = match fetcher.first(&file.rel) {
            Ok(first) => first,
            Err(ViewError::OldHost) => return Ok(Opened::CopyOnly { kind }),
            Err(error) => return Err(error),
        };
        let size = first.size;
        let plan = |reason| {
            Opened::Download(DownloadPlan {
                size,
                kind,
                reason,
                needs_confirm: size > super::view_cache::CONFIRM_BYTES,
            })
        };
        if !kind.previewable() {
            return Ok(plan(PlanReason::NotPreviewable));
        }
        if size > MAX_MEMORY_BYTES {
            return Ok(plan(PlanReason::TooLarge));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        fetcher.stream(&file.rel, first, cancel, &mut |part| {
            bytes.extend_from_slice(part);
            Ok(())
        })?;
        Ok(match kind {
            FileKind::Image => match image_mime(file.name()) {
                Some(mime) => Opened::Image { bytes, mime },
                None => plan(PlanReason::NotPreviewable),
            },
            FileKind::Markdown | FileKind::Text => {
                if bytes.contains(&0) {
                    plan(PlanReason::NotPreviewable)
                } else {
                    Opened::Text {
                        kind,
                        text: String::from_utf8_lossy(bytes.as_slice()).into_owned(),
                    }
                }
            }
            FileKind::Html => {
                let html = String::from_utf8_lossy(bytes.as_slice()).into_owned();
                let base = file.rel.rsplit_once('/').map_or("", |(dir, _)| dir);
                let mut inliner = Inliner {
                    fetcher: &fetcher,
                    root: &file.root,
                    cancel,
                    budget: MAX_INLINE_BYTES.saturating_sub(html.len() as u64),
                    files: RefCell::new(HashMap::new()),
                };
                match inliner.inline_html(&html, base) {
                    Ok(html) => Opened::Html { html },
                    Err(Stop::TooLarge) => plan(PlanReason::HtmlAssets),
                    Err(Stop::Fatal(error)) => return Err(error),
                }
            }
            FileKind::Other => plan(PlanReason::NotPreviewable),
        })
    }
}

/// A referenced file: its workspace path and (shared, not copied) bytes.
type Loaded = (String, Rc<Vec<u8>>);

/// What ends an HTML inlining pass early.
pub(crate) enum Stop {
    /// The assets exceed the in-memory budget.
    TooLarge,
    Fatal(ViewError),
}

/// A reference found in a page or stylesheet that points at a workspace file.
pub(crate) fn local_reference(reference: &str) -> Option<String> {
    let trimmed = reference.trim();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("//")
        || [
            "data:",
            "http:",
            "https:",
            "javascript:",
            "mailto:",
            "about:",
            "blob:",
            "file:",
        ]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return None;
    }
    let end = trimmed.find(['?', '#']).unwrap_or(trimmed.len());
    let path = percent_decode(&trimmed[..end]);
    (!path.is_empty()).then_some(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let decoded = if bytes[at] == b'%' && at + 2 < bytes.len() {
            std::str::from_utf8(&bytes[at + 1..at + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        } else {
            None
        };
        if let Some(byte) = decoded {
            out.push(byte);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Workspace-relative path of `reference` found in a file inside `base_dir` (relative to the
/// root), or `None` when it is external or leaves the workspace.
pub(crate) fn reference_target(root: &str, base_dir: &str, reference: &str) -> Option<String> {
    let path = local_reference(reference)?;
    let absolute = if path.starts_with('/') {
        path
    } else {
        format!("{}/{}/{path}", root.trim_end_matches('/'), base_dir)
    };
    resolve_workspace_path(&absolute, root).ok()
}

fn data_mime(rel: &str) -> &'static str {
    match rel
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("bmp") => "image/bmp",
        Some("avif") => "image/avif",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Inlines the relative assets of one page as `data:` URIs and inline `<script>`/`<style>`.
struct Inliner<'a> {
    fetcher: &'a Fetcher<'a>,
    root: &'a str,
    cancel: &'a AtomicBool,
    /// Bytes still allowed to be added to the page.
    budget: u64,
    files: RefCell<HashMap<String, Option<Rc<Vec<u8>>>>>,
}

impl Inliner<'_> {
    /// The bytes of a referenced file, or `None` to leave the reference as it is (external,
    /// outside the workspace, missing, unreadable).
    fn load(&mut self, base_dir: &str, reference: &str) -> Result<Option<Loaded>, Stop> {
        let Some(rel) = reference_target(self.root, base_dir, reference) else {
            return Ok(None);
        };
        let cached = self.files.borrow().get(&rel).cloned(); // an Rc clone, not the bytes
        let bytes = match cached {
            Some(found) => found,
            None => {
                let fetched = match self.fetcher.read_all(&rel, self.budget, self.cancel) {
                    Ok(Ok(bytes)) => Some(Rc::new(bytes)),
                    Ok(Err(_)) => return Err(Stop::TooLarge),
                    Err(ViewError::Failed(_) | ViewError::Refused(_) | ViewError::OldHost) => None,
                    Err(error) => return Err(Stop::Fatal(error)),
                };
                self.files.borrow_mut().insert(rel.clone(), fetched.clone());
                fetched
            }
        };
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        // Charge what lands in the page: base64 grows the bytes by a third.
        let cost = (bytes.len() as u64).div_ceil(3) * 4;
        if cost > self.budget {
            return Err(Stop::TooLarge);
        }
        self.budget -= cost;
        Ok(Some((rel, bytes)))
    }

    fn data_uri(&mut self, base_dir: &str, reference: &str) -> Result<Option<String>, Stop> {
        Ok(self.load(base_dir, reference)?.map(|(rel, bytes)| {
            format!(
                "data:{};base64,{}",
                data_mime(&rel),
                base64::engine::general_purpose::STANDARD.encode(bytes.as_slice())
            )
        }))
    }

    /// `url(...)` references of a stylesheet that lives in `base_dir`.
    fn inline_css(&mut self, css: &str, base_dir: &str) -> Result<String, Stop> {
        let pattern = &*URL_RE;
        let mut out = String::with_capacity(css.len());
        let mut last = 0;
        for caps in pattern.captures_iter(css) {
            let whole = caps.get(0).expect("whole match");
            let reference = (1..=3).find_map(|i| caps.get(i)).map_or("", |m| m.as_str());
            out.push_str(&css[last..whole.start()]);
            match self.data_uri(base_dir, reference)? {
                Some(uri) => out.push_str(&format!("url(\"{uri}\")")),
                None => out.push_str(whole.as_str()),
            }
            last = whole.end();
        }
        out.push_str(&css[last..]);
        Ok(out)
    }

    fn inline_html(&mut self, html: &str, base_dir: &str) -> Result<String, Stop> {
        let styled = self.inline_styles(html, base_dir)?;
        self.inline_tags(&styled, base_dir)
    }

    /// `<style>` blocks and `style="..."` attributes.
    fn inline_styles(&mut self, html: &str, base_dir: &str) -> Result<String, Stop> {
        let block = &*STYLE_BLOCK_RE;
        let mut out = String::with_capacity(html.len());
        let mut last = 0;
        for caps in block.captures_iter(html) {
            let whole = caps.get(0).expect("whole match");
            out.push_str(&html[last..whole.start()]);
            out.push_str(&caps[1]);
            out.push_str(&self.inline_css(&caps[2], base_dir)?);
            out.push_str(&caps[3]);
            last = whole.end();
        }
        out.push_str(&html[last..]);
        let attribute = &*STYLE_ATTR_RE;
        let mut result = String::with_capacity(out.len());
        let mut last = 0;
        for caps in attribute.captures_iter(&out) {
            let whole = caps.get(0).expect("whole match");
            let value = caps.get(3).or(caps.get(4)).map_or("", |m| m.as_str());
            let quote = if caps.get(3).is_some() { '"' } else { '\'' };
            result.push_str(&out[last..whole.start()]);
            result.push_str(&caps[1]);
            let inlined = self.inline_css(value, base_dir)?;
            result.push(quote);
            // A double quote inside the data URI form would end the attribute.
            result.push_str(&inlined.replace(quote, if quote == '"' { "&quot;" } else { "&#39;" }));
            result.push(quote);
            last = whole.end();
        }
        result.push_str(&out[last..]);
        Ok(result)
    }

    /// `<img>`, `<source>`, `<video>`, `<audio>`, `<link>` and `<script src>` tags.
    fn inline_tags(&mut self, html: &str, base_dir: &str) -> Result<String, Stop> {
        let tag = &*TAG_RE;
        let mut out = String::with_capacity(html.len());
        let mut last = 0;
        let mut search = 0;
        while let Some(caps) = tag.captures_at(html, search) {
            let whole = caps.get(0).expect("whole match");
            search = whole.end();
            if whole.start() < last {
                continue;
            }
            let name = caps[1].to_ascii_lowercase();
            let attrs = caps.get(2).map_or("", |m| m.as_str());
            out.push_str(&html[last..whole.start()]);
            last = whole.end();
            match name.as_str() {
                "script" => {
                    let Some((reference, stripped)) = take_attribute(attrs, "src") else {
                        out.push_str(whole.as_str());
                        continue;
                    };
                    let Some(close) = find_close(html, whole.end(), "</script") else {
                        out.push_str(whole.as_str());
                        continue;
                    };
                    let Some((_, bytes)) = self.load(base_dir, &reference)? else {
                        out.push_str(whole.as_str());
                        continue;
                    };
                    let code =
                        String::from_utf8_lossy(bytes.as_slice()).replace("</script", "<\\/script");
                    out.push_str(&format!("<script{stripped}>{code}</script>"));
                    last = close;
                    search = close;
                }
                "link" => {
                    let stylesheet = attribute_value(attrs, "rel")
                        .is_some_and(|rel| rel.to_ascii_lowercase().contains("stylesheet"));
                    let Some(reference) = attribute_value(attrs, "href") else {
                        out.push_str(whole.as_str());
                        continue;
                    };
                    if stylesheet {
                        let css_dir = reference_target(self.root, base_dir, &reference)
                            .and_then(|rel| rel.rsplit_once('/').map(|(d, _)| d.to_owned()))
                            .unwrap_or_default();
                        match self.load(base_dir, &reference)? {
                            Some((_, bytes)) => {
                                let css = String::from_utf8_lossy(bytes.as_slice()).into_owned();
                                let css = self.inline_css(&css, &css_dir)?;
                                out.push_str(&format!(
                                    "<style>{}</style>",
                                    css.replace("</style", "<\\/style")
                                ));
                            }
                            None => out.push_str(whole.as_str()),
                        }
                    } else {
                        out.push_str(&self.replace_attributes(
                            whole.as_str(),
                            attrs,
                            &["href"],
                            base_dir,
                        )?);
                    }
                }
                _ => out.push_str(&self.replace_attributes(
                    whole.as_str(),
                    attrs,
                    &["src", "poster"],
                    base_dir,
                )?),
            }
        }
        out.push_str(&html[last..]);
        Ok(out)
    }

    /// `tag` with each of `names` that points at a workspace file turned into a data URI.
    fn replace_attributes(
        &mut self,
        tag: &str,
        attrs: &str,
        names: &[&str],
        base_dir: &str,
    ) -> Result<String, Stop> {
        let mut result = tag.to_owned();
        for name in names {
            let Some(reference) = attribute_value(attrs, name) else {
                continue;
            };
            if let Some(uri) = self.data_uri(base_dir, &reference)? {
                result = replace_attribute(&result, name, &uri);
            }
        }
        Ok(result)
    }
}

fn attribute_regex(name: &str) -> &'static regex::Regex {
    ATTRIBUTE_RES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, re)| re)
        .expect("known attribute name")
}

/// Value of attribute `name` in the attribute text of one tag.
fn attribute_value(attrs: &str, name: &str) -> Option<String> {
    let caps = attribute_regex(name).captures(attrs)?;
    (1..=3)
        .find_map(|i| caps.get(i))
        .map(|m| m.as_str().to_owned())
}

/// The value of `name` and the attribute text without it.
fn take_attribute(attrs: &str, name: &str) -> Option<(String, String)> {
    let re = attribute_regex(name);
    let caps = re.captures(attrs)?;
    let whole = caps.get(0)?;
    let value = (1..=3).find_map(|i| caps.get(i))?.as_str().to_owned();
    let mut rest = String::from(&attrs[..whole.start()]);
    rest.push_str(&attrs[whole.end()..]);
    Some((value, rest))
}

fn replace_attribute(tag: &str, name: &str, value: &str) -> String {
    let re = attribute_regex(name);
    let Some(whole) = re.find(tag) else {
        return tag.to_owned();
    };
    let lead = if tag[whole.start()..].starts_with(char::is_whitespace) {
        &tag[whole.start()..whole.start() + 1]
    } else {
        ""
    };
    format!(
        "{}{lead}{name}=\"{value}\"{}",
        &tag[..whole.start()],
        &tag[whole.end()..]
    )
}

/// Byte offset just after the `>` of the first `close` tag (case-insensitive) at or after `from`.
fn find_close(html: &str, from: usize, close: &str) -> Option<usize> {
    let lower = html.get(from..)?.to_ascii_lowercase();
    let at = lower.find(close)?;
    let end = lower[at..].find('>')?;
    Some(from + at + end + 1)
}

/// Every workspace file `html` (a page in `base_dir`) refers to, for a download bundle.
pub(crate) fn html_references(root: &str, base_dir: &str, html: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut add = |reference: &str, dir: &str| {
        if let Some(rel) = reference_target(root, dir, reference) {
            if !found.contains(&rel) {
                found.push(rel);
            }
        }
    };
    let tag = &*TAG_RE;
    for caps in tag.captures_iter(html) {
        let attrs = caps.get(2).map_or("", |m| m.as_str());
        for name in ["src", "href", "poster"] {
            if let Some(reference) = attribute_value(attrs, name) {
                add(&reference, base_dir);
            }
        }
    }
    for reference in css_references(html) {
        add(&reference, base_dir);
    }
    found
}

/// The `url(...)` targets in a stylesheet or a page's style text.
pub(crate) fn css_references(text: &str) -> Vec<String> {
    let pattern = &*URL_RE;
    pattern
        .captures_iter(text)
        .filter_map(|caps| {
            (1..=3)
                .find_map(|i| caps.get(i))
                .map(|m| m.as_str().to_owned())
        })
        .collect()
}
