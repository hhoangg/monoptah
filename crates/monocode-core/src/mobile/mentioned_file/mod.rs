//! `chat.mentionedFile`: a remote device previews a file the assistant mentioned in a chat.
//!
//! This is the one narrow exception to "a remote device reads only inside its selected projects"
//! (security.md section 7, G1/G2): a mentioned file may lie anywhere the desktop user can read,
//! so the rules here are the whole defence, and every one is enforced in this module, in order:
//!
//! 1. **Mention check, first.** The requested path must be one of the paths an ASSISTANT message
//!    of the conversation mentions (full history, see [`mentions`]). User messages never count: a
//!    control device can type any path into one. The comparison is lexical and runs before any
//!    file-system access to the requested path, so an unmentioned path never reveals whether it
//!    exists.
//! 2. **Deny-list** (H-04) on the written path and on the canonical target (symlinks resolved),
//!    case-insensitively, on every component of the absolute path, with the owning project's own
//!    patterns when the file lies in a project; plus the app's data folders and a few credential
//!    folders under the home folder.
//! 3. **No directories**, and nothing that is not a regular file.
//! 4. **Budgets** per device (calls and bytes, see [`budget`]), one reply at most
//!    [`MENTIONED_FILE_PAGE_BYTES`] raw bytes, one file at most [`MENTIONED_FILE_MAX_BYTES`].
//! 5. **Assets** (images an HTML or markdown file shows) are reached only relative to the
//!    mentioned file's folder, never through `..`, an absolute path, a scheme, a backslash or a
//!    symlink out of that folder, and only as images.
//!
//! The caller audits (rule 5 of the contract) with [`Call`], which carries what the line needs.
pub mod budget;
pub mod html;
pub mod mentions;

use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

use super::{
    audit,
    denylist::DenyList,
    policy::{Code, Failure},
};
use crate::files::{links::link_candidates, path_key, remote::is_remote};
pub use mentions::extract_mentions;

/// Largest slice of a file one reply carries, in raw bytes.
pub const MENTIONED_FILE_PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Largest file served (an HTML file's transformed document counts the source).
pub const MENTIONED_FILE_MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Longest `path` or `asset`, in characters.
pub const MAX_PATH_CHARS: usize = 1024;
/// One linked stylesheet, and all of them together, in bytes.
const MAX_SHEET_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SHEETS_TOTAL: u64 = 8 * 1024 * 1024;
/// What is read to tell text from binary when the extension does not say.
const SNIFF_BYTES: usize = 8192;
/// Under this a page cannot hold one whole character.
const MIN_PAGE_BYTES: usize = 4;

/// What the device asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Params {
    pub path: String,
    pub asset: Option<String>,
    pub offset: u64,
}

impl Params {
    /// The parameters of a request, or the refusal for a malformed one.
    pub fn from_json(params: &Value) -> Result<Self, Failure> {
        let invalid = || Failure::from("Invalid mentioned file request");
        let text = |value: &Value| -> Result<Option<String>, Failure> {
            match value {
                Value::Null => Ok(None),
                Value::String(s)
                    if !s.is_empty()
                        && s.chars().count() <= MAX_PATH_CHARS
                        && !s.contains('\0') =>
                {
                    Ok(Some(s.clone()))
                }
                _ => Err(invalid()),
            }
        };
        let path = text(&params["path"])?.ok_or_else(invalid)?;
        let asset = text(&params["asset"])?;
        let offset = match &params["offset"] {
            Value::Null => 0,
            value => value.as_u64().ok_or_else(invalid)?,
        };
        Ok(Self {
            path,
            asset,
            offset,
        })
    }
}

/// A project folder the desktop knows, for the deny-list's patterns and the reply's `project`.
#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub root: PathBuf,
    canonical: PathBuf,
    /// The device selected this project (its `project` field is only set for these).
    pub selected: bool,
    pub deny: DenyList,
}

impl Project {
    pub fn new(id: &str, root: &Path, selected: bool, deny: DenyList) -> Self {
        Self {
            id: id.to_owned(),
            canonical: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            root: root.to_path_buf(),
            selected,
            deny,
        }
    }
}

/// What the call is judged against.
pub struct Context<'a> {
    /// The text of every ASSISTANT message of the conversation, older pages included.
    pub assistant_texts: &'a [String],
    /// The session's working folder (empty for a Mono that has not talked yet).
    pub cwd: &'a str,
    /// A Mono's local project folders, for a relative mention.
    pub roots: &'a [String],
    pub projects: &'a [Project],
    /// The user's home folder, for the credential folders under it.
    pub home: Option<PathBuf>,
    /// The app's own data folders, never served.
    pub private_dirs: Vec<PathBuf>,
    pub page_bytes: usize,
    pub max_bytes: u64,
}

impl<'a> Context<'a> {
    pub fn new(
        assistant_texts: &'a [String],
        cwd: &'a str,
        roots: &'a [String],
        projects: &'a [Project],
    ) -> Self {
        Self {
            assistant_texts,
            cwd,
            roots,
            projects,
            home: default_home(),
            private_dirs: Vec::new(),
            page_bytes: MENTIONED_FILE_PAGE_BYTES,
            max_bytes: MENTIONED_FILE_MAX_BYTES,
        }
    }
}

/// The user's home folder, when the platform names one.
pub fn default_home() -> Option<PathBuf> {
    let home = crate::scm::expand_home(Path::new("~"));
    (home != Path::new("~")).then_some(home)
}

/// How the client shows the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Markdown,
    Html,
    Image,
    Binary,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Markdown => "markdown",
            Kind::Html => "html",
            Kind::Image => "image",
            Kind::Binary => "binary",
        }
    }
}

/// The reply of one call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub kind: Kind,
    pub mime: Option<&'static str>,
    /// `utf8`, `base64` or `none`.
    pub encoding: &'static str,
    pub data: String,
    pub offset: u64,
    pub total: u64,
    pub next_offset: Option<u64>,
    pub assets: Option<Vec<String>>,
    pub project: Option<(String, String)>,
    /// Raw bytes this reply carried, for the budget and the audit.
    pub served: u64,
}

impl Reply {
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "path": self.path,
            "name": self.name,
            "size": self.size,
            "kind": self.kind.as_str(),
            "encoding": self.encoding,
            "data": self.data,
            "offset": self.offset,
            "total": self.total,
        });
        if let Some(mime) = self.mime {
            out["mime"] = mime.into();
        }
        if let Some(next) = self.next_offset {
            out["nextOffset"] = next.into();
        }
        if let Some(assets) = &self.assets {
            out["assets"] = json!(assets);
        }
        if let Some((id, path)) = &self.project {
            out["project"] = json!({"projectId": id, "path": path});
        }
        out
    }
}

/// One call, answered: the result and what its audit line needs.
pub struct Call {
    pub result: Result<Value, Failure>,
    /// The canonical path served, or the path as requested when refused.
    pub path: String,
    pub asset: Option<String>,
    /// Raw bytes served.
    pub bytes: u64,
}

impl Call {
    pub fn outcome(&self) -> audit::Outcome {
        match &self.result {
            Ok(_) => audit::Outcome::Ok,
            Err(failure) => super::policy::code_of_failure(failure)
                .map_or(audit::Outcome::Error, audit::Outcome::Denied),
        }
    }
}

/// The whole method for `device`: the budget, the parameters, [`serve`], the bytes tally. The
/// app and the headless E2E desktop both answer through it.
pub fn handle(device: &str, params: &Value, ctx: &Context) -> Call {
    let requested = params["path"].as_str().unwrap_or_default().to_owned();
    let asset = params["asset"].as_str().map(str::to_owned);
    let refuse = |failure: Failure| Call {
        result: Err(failure),
        path: requested.clone(),
        asset: asset.clone(),
        bytes: 0,
    };
    if let Err(code) = budget::admit(device) {
        return refuse(code.failure());
    }
    let parsed = match Params::from_json(params) {
        Ok(parsed) => parsed,
        Err(failure) => return refuse(failure),
    };
    match serve(&parsed, ctx) {
        Ok(reply) => {
            budget::served(device, reply.served);
            Call {
                path: reply.path.clone(),
                asset,
                bytes: reply.served,
                result: Ok(reply.to_json()),
            }
        }
        Err(failure) => refuse(failure),
    }
}

/// The text of every assistant message in `rows`. Only these may authorize a path: a user
/// message (or a Mono system prompt) is text a remote device can write itself.
pub fn assistant_texts<'a>(
    rows: impl IntoIterator<Item = &'a crate::transcript::Row>,
) -> Vec<String> {
    rows.into_iter()
        .filter(|row| row.role == "assistant" && !row.text.is_empty())
        .map(|row| row.text.clone())
        .collect()
}

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn denied() -> Failure {
    Code::PathDenied.failure()
}

fn forbidden() -> Failure {
    Code::Forbidden.failure()
}

/// Everything the deny-list and the protected folders say about a path.
#[derive(Clone)]
struct Guard {
    projects: Vec<(PathBuf, PathBuf, DenyList)>,
    default: DenyList,
    protected: Vec<Vec<String>>,
}

fn lower_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect()
}

impl Guard {
    fn new(ctx: &Context) -> Self {
        let mut protected: Vec<Vec<String>> = Vec::new();
        let mut protect = |path: &Path| {
            for variant in [Some(path.to_path_buf()), path.canonicalize().ok()]
                .into_iter()
                .flatten()
            {
                let parts = lower_components(&variant);
                if !parts.is_empty() && !protected.contains(&parts) {
                    protected.push(parts);
                }
            }
        };
        for dir in &ctx.private_dirs {
            protect(dir);
        }
        if let Some(home) = &ctx.home {
            for tail in [
                ".config/monoptah",
                ".config/gh",
                "Library/Keychains",
                "Library/Cookies",
            ] {
                protect(&home.join(tail));
            }
        }
        Self {
            projects: ctx
                .projects
                .iter()
                .map(|p| (p.root.clone(), p.canonical.clone(), p.deny.clone()))
                .collect(),
            default: DenyList::default(),
            protected,
        }
    }

    fn list_for(&self, path: &Path) -> &DenyList {
        self.projects
            .iter()
            .filter(|(raw, canonical, _)| path.starts_with(raw) || path.starts_with(canonical))
            .max_by_key(|(_, canonical, _)| canonical.as_os_str().len())
            .map_or(&self.default, |(_, _, list)| list)
    }

    /// Whether `path` (absolute, written or canonical) may not be served.
    fn denied(&self, path: &Path) -> bool {
        if self.list_for(path).is_denied_rel(&lossy(path)) {
            return true;
        }
        let parts = lower_components(path);
        self.protected
            .iter()
            .any(|prefix| parts.starts_with(prefix))
    }
}

/// The paths of the file mentions in `texts`, as lexical candidates (no disk access).
fn mentioned_keys(ctx: &Context) -> HashSet<String> {
    let mut keys = HashSet::new();
    for text in ctx.assistant_texts {
        for mention in extract_mentions(text) {
            for candidate in link_candidates(&mention, ctx.cwd, ctx.roots) {
                keys.insert(path_key(&lossy(&candidate)));
            }
        }
    }
    keys
}

/// Rule 1: the candidates of the requested path that an assistant message mentions. Nothing here
/// touches the disk.
fn authorized(ctx: &Context, requested: &str) -> Vec<PathBuf> {
    let keys = mentioned_keys(ctx);
    link_candidates(requested, ctx.cwd, ctx.roots)
        .into_iter()
        .filter(|path| path.is_absolute() && !is_remote(path))
        .filter(|path| keys.contains(&path_key(&lossy(path))))
        .collect()
}

/// Answers one call. See the module docs for the rules, in the order they are applied.
pub fn serve(params: &Params, ctx: &Context) -> Result<Reply, Failure> {
    let guard = Guard::new(ctx);
    let page = ctx.page_bytes.max(MIN_PAGE_BYTES);
    // 1. The mention check, before the disk is touched.
    let candidates = authorized(ctx, &params.path);
    if candidates.is_empty() {
        return Err(forbidden());
    }
    // 2. The written path against the deny-list, still without the disk.
    let allowed: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|path| !guard.denied(path))
        .collect();
    if allowed.is_empty() {
        return Err(denied());
    }
    // The first candidate that exists, canonicalised (symlinks resolved).
    let canonical = allowed
        .iter()
        .find_map(|path| path.canonicalize().ok())
        .ok_or_else(|| Code::NotFound.failure())?;
    if guard.denied(&canonical) {
        return Err(denied());
    }
    if let Some(project) = ctx
        .projects
        .iter()
        .find(|p| canonical.starts_with(&p.canonical))
    {
        project
            .deny
            .check(&project.canonical, &canonical)
            .map_err(|_| denied())?;
    }
    // 3. Never a directory, nor a device or pipe (checked before opening: opening a pipe blocks).
    let metadata = std::fs::metadata(&canonical).map_err(|_| Code::NotFound.failure())?;
    if !metadata.is_file() {
        return Err(forbidden());
    }
    let mut file = File::open(&canonical).map_err(|_| Code::NotFound.failure())?;
    match &params.asset {
        None => serve_file(
            &mut file,
            &canonical,
            metadata.len(),
            params,
            ctx,
            &guard,
            page,
        ),
        Some(asset) => serve_asset(&canonical, asset, params, ctx, &guard, page),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The reply's `project`: the selected project holding `path`, and the path inside it.
fn project_of(ctx: &Context, path: &Path) -> Option<(String, String)> {
    let project = ctx
        .projects
        .iter()
        .filter(|p| p.selected && path.starts_with(&p.canonical))
        .max_by_key(|p| p.canonical.as_os_str().len())?;
    let relative = path.strip_prefix(&project.canonical).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some((project.id.clone(), parts.join("/")))
}

fn mime_of(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => return None,
    })
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "log",
    "json",
    "jsonc",
    "json5",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "csv",
    "tsv",
    "xml",
    "rs",
    "ts",
    "tsx",
    "js",
    "jsx",
    "mjs",
    "cjs",
    "py",
    "rb",
    "go",
    "java",
    "kt",
    "swift",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "php",
    "sh",
    "bash",
    "zsh",
    "fish",
    "sql",
    "css",
    "scss",
    "less",
    "vue",
    "svelte",
    "lua",
    "pl",
    "r",
    "dart",
    "gradle",
    "properties",
    "lock",
    "diff",
    "patch",
    "tex",
    "rst",
    "adoc",
    "org",
    "gitignore",
    "dockerfile",
    "mk",
];

const BINARY_EXTENSIONS: &[&str] = &[
    "pdf", "zip", "gz", "tgz", "bz2", "xz", "7z", "tar", "rar", "exe", "dll", "so", "dylib", "bin",
    "dmg", "iso", "mp3", "mp4", "mov", "avi", "mkv", "wav", "flac", "ogg", "woff", "woff2", "ttf",
    "otf", "class", "jar", "o", "a", "wasm", "sqlite", "db", "psd", "doc", "docx", "xls", "xlsx",
    "ppt", "pptx",
];

/// The kind an extension fixes, or `None` when the content has to say.
fn kind_by_extension(name: &str) -> Option<Kind> {
    let extension = html::extension(name)?;
    let extension = extension.as_str();
    Some(match extension {
        "html" | "htm" => Kind::Html,
        "md" | "markdown" | "mdx" => Kind::Markdown,
        _ if mime_of(extension).is_some() => Kind::Image,
        _ if TEXT_EXTENSIONS.contains(&extension) => Kind::Text,
        _ if BINARY_EXTENSIONS.contains(&extension) => Kind::Binary,
        _ => return None,
    })
}

/// Whether the start of a file reads as UTF-8 text: no NUL, and valid but for a character cut
/// at the end of what was read.
fn looks_like_text(head: &[u8], complete: bool) -> bool {
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(error) => !complete && error.error_len().is_none(),
    }
}

fn read_at(file: &mut File, offset: u64, len: usize) -> Result<Vec<u8>, Failure> {
    let failed = |_| Failure::from("Could not read the file");
    file.seek(SeekFrom::Start(offset)).map_err(failed)?;
    let mut buffer = Vec::with_capacity(len.min(1 << 20));
    file.take(len as u64)
        .read_to_end(&mut buffer)
        .map_err(failed)?;
    Ok(buffer)
}

fn serve_file(
    file: &mut File,
    canonical: &Path,
    size: u64,
    params: &Params,
    ctx: &Context,
    guard: &Guard,
    page: usize,
) -> Result<Reply, Failure> {
    let name = file_name(canonical);
    let kind = match kind_by_extension(&name) {
        Some(kind) => kind,
        None => {
            let head = read_at(file, 0, SNIFF_BYTES)?;
            let complete = (head.len() as u64) >= size;
            if looks_like_text(&head, complete) {
                Kind::Text
            } else {
                Kind::Binary
            }
        }
    };
    let mut reply = Reply {
        path: lossy(canonical),
        name,
        size,
        kind,
        mime: None,
        encoding: "utf8",
        data: String::new(),
        offset: 0,
        total: size,
        next_offset: None,
        assets: None,
        project: project_of(ctx, canonical),
        served: 0,
    };
    if kind == Kind::Binary {
        // Size and path only: no content leaves for a file the client cannot show.
        reply.encoding = "none";
        return Ok(reply);
    }
    if size > ctx.max_bytes {
        return Err(Code::TooLarge.failure());
    }
    match kind {
        Kind::Text | Kind::Markdown => text_page(file, &mut reply, params.offset, page)?,
        Kind::Image => {
            reply.mime = html::extension(&reply.name).and_then(|e| mime_of(&e));
            image_page(file, &mut reply, params.offset, page)?;
        }
        Kind::Html => {
            let raw = read_at(file, 0, size as usize)?;
            let source = String::from_utf8_lossy(&raw).into_owned();
            let folder = canonical.parent().unwrap_or(canonical).to_path_buf();
            let sheets = Arc::new(FolderSheets {
                folder,
                guard: guard.clone(),
                used: AtomicU64::new(0),
            });
            let document = html::transform(&source, sheets);
            reply.assets = Some(document.assets);
            string_page(&mut reply, &document.html, params.offset, page);
        }
        Kind::Binary => {}
    }
    Ok(reply)
}

/// The largest prefix of `buffer` that ends on a UTF-8 character boundary, assuming the text
/// before it is sound.
fn complete_prefix(buffer: &[u8]) -> usize {
    let len = buffer.len();
    for back in 1..=len.min(3) {
        let byte = buffer[len - back];
        if byte & 0xC0 == 0x80 {
            continue;
        }
        if byte >= 0xC0 {
            let needed = if byte >= 0xF0 {
                4
            } else if byte >= 0xE0 {
                3
            } else {
                2
            };
            return if back >= needed { len } else { len - back };
        }
        return len;
    }
    len
}

fn text_page(file: &mut File, reply: &mut Reply, offset: u64, page: usize) -> Result<(), Failure> {
    let size = reply.size;
    let mut start = offset.min(size);
    let mut buffer = read_at(file, start, page)?;
    // An offset inside a character moves on to the next one.
    if start > 0 {
        let skip = buffer
            .iter()
            .take(3)
            .take_while(|byte| **byte & 0xC0 == 0x80)
            .count();
        buffer.drain(..skip);
        start += skip as u64;
    }
    if start + (buffer.len() as u64) < size {
        buffer.truncate(complete_prefix(&buffer));
    }
    reply.offset = start;
    reply.served = buffer.len() as u64;
    reply.data = String::from_utf8_lossy(&buffer).into_owned();
    let end = start + buffer.len() as u64;
    reply.next_offset = (end < size && !buffer.is_empty()).then_some(end);
    Ok(())
}

fn image_page(file: &mut File, reply: &mut Reply, offset: u64, page: usize) -> Result<(), Failure> {
    let size = reply.size;
    let start = offset.min(size);
    let buffer = read_at(file, start, page)?;
    reply.encoding = "base64";
    reply.offset = start;
    reply.served = buffer.len() as u64;
    reply.data = STANDARD.encode(&buffer);
    let end = start + buffer.len() as u64;
    reply.next_offset = (end < size && !buffer.is_empty()).then_some(end);
    Ok(())
}

/// A page of a string that is already in memory (the transformed HTML), cut on character
/// boundaries.
fn string_page(reply: &mut Reply, text: &str, offset: u64, page: usize) {
    let total = text.len();
    let mut start = (offset.min(total as u64)) as usize;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + page).min(total);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    reply.total = total as u64;
    reply.offset = start as u64;
    reply.data = text[start..end].to_owned();
    reply.served = (end - start) as u64;
    reply.next_offset = (end < total && end > start).then_some(end as u64);
}

/// Rule 5: an image the mentioned HTML or markdown file shows, inside its folder.
fn serve_asset(
    parent: &Path,
    asset: &str,
    params: &Params,
    ctx: &Context,
    guard: &Guard,
    page: usize,
) -> Result<Reply, Failure> {
    // Only a page that shows images has assets.
    if !matches!(
        kind_by_extension(&file_name(parent)),
        Some(Kind::Html | Kind::Markdown)
    ) {
        return Err(forbidden());
    }
    let lower = asset.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "%00"]
        .iter()
        .any(|bad| lower.contains(bad))
    {
        return Err(forbidden());
    }
    let relative = html::normalize_asset(asset).ok_or_else(forbidden)?;
    let folder = parent.parent().ok_or_else(forbidden)?;
    let joined = folder.join(&relative);
    if guard.denied(&joined) {
        return Err(denied());
    }
    let canonical = joined
        .canonicalize()
        .map_err(|_| Code::NotFound.failure())?;
    if !canonical.starts_with(folder) {
        return Err(forbidden());
    }
    if guard.denied(&canonical) {
        return Err(denied());
    }
    let metadata = std::fs::metadata(&canonical).map_err(|_| Code::NotFound.failure())?;
    if !metadata.is_file() {
        return Err(forbidden());
    }
    let mut file = File::open(&canonical).map_err(|_| Code::NotFound.failure())?;
    let name = file_name(&canonical);
    // The extension of the canonical target decides, so a link named `a.png` cannot serve a
    // different kind of file.
    let mime = html::extension(&name)
        .and_then(|e| mime_of(&e))
        .ok_or_else(forbidden)?;
    if metadata.len() > ctx.max_bytes {
        return Err(Code::TooLarge.failure());
    }
    let mut reply = Reply {
        path: lossy(&canonical),
        name,
        size: metadata.len(),
        kind: Kind::Image,
        mime: Some(mime),
        encoding: "base64",
        data: String::new(),
        offset: 0,
        total: metadata.len(),
        next_offset: None,
        assets: None,
        project: project_of(ctx, &canonical),
        served: 0,
    };
    image_page(&mut file, &mut reply, params.offset, page)?;
    Ok(reply)
}

/// The linked stylesheets of a mentioned HTML file: local, inside its folder, not denied.
struct FolderSheets {
    folder: PathBuf,
    guard: Guard,
    used: AtomicU64,
}

impl html::Sheets for FolderSheets {
    fn load(&self, href: &str) -> Option<String> {
        let reference = href.split(['?', '#']).next()?;
        let relative = html::normalize_relative(&html::percent_decode(reference))?;
        if html::extension(&relative).as_deref() != Some("css") {
            return None;
        }
        let joined = self.folder.join(relative);
        if self.guard.denied(&joined) {
            return None;
        }
        let canonical = joined.canonicalize().ok()?;
        if !canonical.starts_with(&self.folder) || self.guard.denied(&canonical) {
            return None;
        }
        let mut file = File::open(&canonical).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_SHEET_BYTES {
            return None;
        }
        if self.used.fetch_add(metadata.len(), Ordering::SeqCst) + metadata.len() > MAX_SHEETS_TOTAL
        {
            return None;
        }
        let mut text = Vec::new();
        file.read_to_end(&mut text).ok()?;
        Some(String::from_utf8_lossy(&text).into_owned())
    }
}

// The tests build POSIX paths (`/tmp/...`, `~/`), which is what mentions look like on the
// desktops that serve this method.
#[cfg(all(test, unix))]
mod tests;
