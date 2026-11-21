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
//!    exists. It is then exact: the requested path and a mention must resolve (as the desktop's
//!    click resolves them, `files::links::resolve_link`) to the same canonical file, so a
//!    relative mention authorizes one file, not that name under every root.
//! 2. **Evidence outside the device's projects.** A file outside every project the device
//!    selected also needs one of these (see [`evidence`]), because an assistant message alone can
//!    be provoked by a control device ("reply with `~/x`"):
//!    1. a COMPLETED file tool (Read, Write, Edit, patch) of the conversation named it through a
//!       structured path parameter;
//!    2. a COMPLETED shell call (its command line or output) or a COMPLETED subagent call (its
//!       output) names it. No other tool counts, nor a call that did not complete. This is
//!       what lets a report a script wrote be opened;
//!    3. the file was changed after the conversation started (its inode change time, or its
//!       modification time off unix; a modification time in the future never counts) and lies
//!       inside the session's own folder: its live Monoptah worktree, else its working folder.
//!       Only for a session, never for a Mono (its start can be months old). That folder is no
//!       tree when it is the home folder, a filesystem root or an ancestor of the home folder,
//!       when it lies in (or is a worktree of) a known project the device did not select, or when
//!       the home folder is unknown. A file inside a known project the device did not select is
//!       never fresh, whatever folder the session runs in. A plain working folder that is no
//!       project (`~/Downloads`) does count. The start is [`Context::started`] (the session's
//!       creation time), else the earliest plausible send time of the history.
//!
//!    Loose evidence comes from shell command lines and outputs and from subagent outputs only
//!    (see [`evidence`]). A control device can ask the agent to run a shell command that names a
//!    path (in auto or bypass mode no approval is asked) and shell output can carry injected
//!    text; and the freshness rule covers anything any process changed in the session's own
//!    folder since the session began. That residual is accepted: the deny-list (rule 3) and the protected folders
//!    are the last defence, see security.md section 7.
//!
//! 3. **Deny-list** (H-04) on the written path and on the canonical target (symlinks resolved),
//!    case-insensitively, on every component of the absolute path, with the owning project's own
//!    patterns when the file lies in a project or in the session's worktree of one; plus the app's data folders and the credential
//!    folders under the home folder, and a hard link to a credential file.
//! 4. **No directories**, and nothing that is not a regular file. The file is opened without
//!    following a link and without blocking, and the opened handle must be the checked file.
//! 5. **Budgets** per device (calls and bytes, see [`budget`]), one reply at most
//!    [`MENTIONED_FILE_PAGE_BYTES`] raw bytes (less for text, see [`MENTIONED_FILE_UTF8_PAGE_BYTES`]),
//!    one file at most [`MENTIONED_FILE_MAX_BYTES`], one HTML transform at a time per device.
//! 6. **Assets** (images an HTML or markdown file shows) are reached only when that file
//!    references them (the exact key it lists), relative to its folder or as an absolute or
//!    `file://` path, and only inside the root the file was authorized under: the selected
//!    project holding it, else its own folder. Checked by name first and again after the links
//!    are followed, never through a scheme or a backslash, and only as images. The file itself
//!    qualified under rules 1 to 5 by whichever evidence; its assets need nothing more.
//!
//! The caller audits (rule 5 of the contract) with [`Call`], which carries what the line needs,
//! including which rule refused ([`rule`]).
pub mod budget;
pub mod cost;
pub mod evidence;
pub mod html;
pub mod mentions;
pub mod prepass;
pub mod sandbox;
pub mod transfer;

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, LazyLock, Mutex,
    },
    time::{Duration, Instant, SystemTime},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

use super::{
    audit,
    denylist::DenyList,
    policy::{Code, Failure},
};
use crate::credential_guard::{
    all_app_data_dirs, file_id, link_count, SecretIndex, INDEXED_HOME_PATHS, MORE_HOME_CREDENTIALS,
};
use crate::files::{
    links::{file_location, link_candidates, resolve_link, written_relative},
    path_key,
    remote::is_remote,
};
pub use evidence::Evidence;
pub use mentions::extract_mentions;

/// Largest slice of a file one reply carries, in raw bytes (images; text is cut shorter).
pub const MENTIONED_FILE_PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Largest slice of text one reply carries, in raw bytes. JSON escaping can grow a control
/// character six times, so text is also cut by its escaped length ([`MAX_DATA_JSON_BYTES`]).
pub const MENTIONED_FILE_UTF8_PAGE_BYTES: usize = 2 * 1024 * 1024;
/// The most a reply's `data` may take once JSON-escaped, so a reply stays under the 8 MiB
/// reassembly cap of the transport.
pub const MAX_DATA_JSON_BYTES: usize = 6 * 1024 * 1024;
/// Largest file served (an HTML file's transformed document is held to the same size).
pub const MENTIONED_FILE_MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Largest HTML or markdown source that is transformed or searched for references.
pub const MAX_REFERENCING_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
/// Elements times style bytes (inline `<style>` plus linked sheets) an HTML document may have:
/// what matching the selectors costs. 5000 elements with 200 KB of CSS is accepted.
pub const MAX_SCAN_WORK: u64 = 1_000_000_000;
/// Declaration bytes times the elements they can reach (see [`cost`]) an HTML document may have:
/// inlining copies a declaration into every element it matches, so this bounds the time and
/// memory of the worst case (measured: about 30 bytes of memory per unit, so about 200 MB).
pub const MAX_COPY_WORK: u64 = 6_000_000;
/// Selector checks (with the walks of descendant, sibling and `:has(` selectors, see [`cost`])
/// an HTML document may cost, whatever its declarations. A pre-filter: the transform is bounded
/// by its child process (see [`sandbox`]).
pub const MAX_MATCH_WORK: u64 = 2_000_000_000;
/// Longest `path` or `asset`, in characters.
pub const MAX_PATH_CHARS: usize = 1024;
/// One linked stylesheet, and all of them together, in bytes.
const MAX_SHEET_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SHEETS_TOTAL: u64 = 8 * 1024 * 1024;
/// What is read to tell text from binary when the extension does not say.
const SNIFF_BYTES: usize = 8192;
/// Under this a page cannot hold one whole character.
const MIN_PAGE_BYTES: usize = 4;
/// Tool-evidence candidates checked on the disk for one call.
const MAX_TOOL_DISK_CHECKS: usize = 200;

/// What the device asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Params {
    pub path: String,
    pub asset: Option<String>,
    pub offset: u64,
    /// For an HTML or markdown file: its raw text instead of the rendered document.
    pub source: bool,
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
        let source = match &params["source"] {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            _ => return Err(invalid()),
        };
        Ok(Self {
            path,
            asset,
            offset,
            source,
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

/// The session's worktree: a folder made by the app for a session of `project`.
#[derive(Clone, Copy, Debug)]
pub struct Worktree<'a> {
    pub path: &'a str,
    /// The project folder the worktree belongs to (the session's own folder).
    pub project: &'a str,
}

/// What the call is judged against.
pub struct Context<'a> {
    /// The text of every ASSISTANT message of the conversation, older pages included. Ignored
    /// when [`Context::evidence`] is set.
    pub assistant_texts: &'a [String],
    /// The text of every TOOL row of the conversation (JSON tool calls or plain text), the
    /// evidence a file outside the device's projects needs. Ignored when
    /// [`Context::evidence`] is set.
    pub tool_texts: &'a [String],
    /// Evidence already extracted (and perhaps cached) by the caller, instead of the texts.
    pub evidence: Option<&'a Evidence>,
    /// The folder a relative mention is resolved against first, as the desktop's click binds
    /// it: the first local project of a Mono that has some, else the session's folder (its
    /// worktree when it has one). Empty for a Mono that has not talked yet. For a session it is
    /// also the tree of the freshness rule (see [`fresh_tree`]).
    pub cwd: &'a str,
    /// The session's Monoptah worktree of one of [`Context::projects`], when it has a live one.
    pub worktree: Option<Worktree<'a>>,
    /// The conversation is a Mono's: the freshness rule never applies to it.
    pub mono: bool,
    /// When the conversation began (the session's creation time), for the freshness rule. When
    /// `None`, the earliest plausible send time of the evidence is used (none: the rule never
    /// applies).
    pub started: Option<SystemTime>,
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
            tool_texts: &[],
            evidence: None,
            cwd,
            worktree: None,
            mono: false,
            started: None,
            roots,
            projects,
            home: default_home(),
            private_dirs: Vec::new(),
            page_bytes: MENTIONED_FILE_PAGE_BYTES,
            max_bytes: MENTIONED_FILE_MAX_BYTES,
        }
    }

    /// When the conversation began, as far as known.
    fn started_at(&self, evidence: &Evidence) -> Option<SystemTime> {
        self.started.or_else(|| {
            evidence
                .started_ms
                .filter(|ms| *ms >= evidence::MIN_PLAUSIBLE_MS)
                .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
        })
    }

    /// The session's worktree (canonical) and the known project that owns it. A worktree whose
    /// project the desktop does not know is none.
    fn worktree_owner(&self) -> Option<(PathBuf, &Project)> {
        let tree = self.worktree?;
        let path = Path::new(tree.path);
        if !path.is_absolute() || is_remote(path) {
            return None;
        }
        let real = path.canonicalize().ok()?;
        let owner = Path::new(tree.project)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(tree.project));
        let project = self.projects.iter().find(|p| p.canonical == owner)?;
        Some((real, project))
    }

    fn evidence(&self) -> Cow<'_, Evidence> {
        match self.evidence {
            Some(evidence) => Cow::Borrowed(evidence),
            None => Cow::Owned(Evidence::from_texts(self.assistant_texts, self.tool_texts)),
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

/// Which rule refused a call, for the audit line only (the device gets the short code of the
/// failure, never this: it must not tell what exists).
pub mod rule {
    pub const BAD_REQUEST: &str = "bad_request";
    pub const BUDGET: &str = "budget";
    pub const NOT_MENTIONED: &str = "not_mentioned";
    pub const NOT_EXACT: &str = "not_exact";
    pub const NO_EVIDENCE: &str = "no_evidence";
    pub const DENY_LIST: &str = "deny_list";
    pub const PROTECTED_DIR: &str = "protected_dir";
    pub const HARD_LINK: &str = "hard_link";
    pub const NOT_REGULAR: &str = "not_regular";
    pub const NOT_FOUND: &str = "not_found";
    pub const TOO_LARGE: &str = "too_large";
    pub const BUSY: &str = "busy";
    pub const ASSET: &str = "asset";
    pub const NOT_IN_PROJECT: &str = "not_in_project";
    pub const FAILED: &str = "failed";
}

/// A refused call: the failure the device gets and the rule behind it, for the audit.
#[derive(Debug)]
pub struct Refusal {
    pub failure: Failure,
    pub rule: &'static str,
}

impl Refusal {
    fn new(failure: Failure, rule: &'static str) -> Self {
        Self { failure, rule }
    }

    /// A refusal from a later stage, named by its code.
    fn of_code(failure: Failure) -> Self {
        let rule = match super::policy::code_of_failure(&failure) {
            Some(Code::NotFound) => rule::NOT_FOUND,
            Some(Code::TooLarge) => rule::TOO_LARGE,
            Some(Code::PathDenied) => rule::DENY_LIST,
            Some(Code::Busy) => rule::BUSY,
            Some(Code::Forbidden) => rule::ASSET,
            _ => rule::FAILED,
        };
        Self { failure, rule }
    }
}

impl From<Failure> for Refusal {
    fn from(failure: Failure) -> Self {
        Self::of_code(failure)
    }
}

/// One call, answered: the result and what its audit line needs.
pub struct Call {
    pub result: Result<Value, Failure>,
    /// The rule that refused the call, when it was refused.
    pub rule: Option<&'static str>,
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

    /// A refusal of the call described by `params`, with nothing served.
    pub fn refused(params: &Value, failure: Failure, rule: &'static str) -> Self {
        Self {
            result: Err(failure),
            rule: Some(rule),
            path: params["path"].as_str().unwrap_or_default().to_owned(),
            asset: params["asset"].as_str().map(str::to_owned),
            bytes: 0,
        }
    }
}

/// The whole method for `device`: the budget, the parameters, [`serve_as`], the bytes tally.
/// The headless E2E desktop answers through it; the app admits first (so it gathers nothing for
/// a device over budget) and calls [`handle_admitted`].
pub fn handle(device: &str, params: &Value, ctx: &Context) -> Call {
    if budget::admit(device).is_err() {
        return Call::refused(params, budget::busy(device, 0), rule::BUDGET);
    }
    handle_admitted(device, params, ctx)
}

/// [`handle`] for a call [`budget::admit`] already counted.
pub fn handle_admitted(device: &str, params: &Value, ctx: &Context) -> Call {
    let asset = params["asset"].as_str().map(str::to_owned);
    let parsed = match Params::from_json(params) {
        Ok(parsed) => parsed,
        Err(failure) => return Call::refused(params, failure, rule::BAD_REQUEST),
    };
    match serve_ruled(device, &parsed, ctx) {
        Ok(reply) => {
            budget::served(device, reply.served);
            Call {
                path: reply.path.clone(),
                asset,
                bytes: reply.served,
                rule: None,
                result: Ok(reply.to_json()),
            }
        }
        Err(refusal) => Call::refused(params, refusal.failure, refusal.rule),
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

/// The text of every tool row in `rows`.
pub fn tool_texts<'a>(rows: impl IntoIterator<Item = &'a crate::transcript::Row>) -> Vec<String> {
    rows.into_iter()
        .filter(|row| row.role == "tool" && !row.text.is_empty())
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

fn not_found() -> Failure {
    Code::NotFound.failure()
}

/// Everything the deny-list and the protected folders say about a path.
#[derive(Clone)]
struct Guard {
    projects: Vec<(PathBuf, PathBuf, DenyList)>,
    default: DenyList,
    protected: Vec<Vec<String>>,
    home: Option<PathBuf>,
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
            // The same credential places the cloud kit guards, plus the browser and keychain
            // folders and every data folder of the app (both flavors).
            for tail in INDEXED_HOME_PATHS
                .iter()
                .chain(MORE_HOME_CREDENTIALS.iter())
                .chain(["Library/Keychains", "Library/Cookies"].iter())
                .chain(crate::credential_guard::SECRET_STORE_DIRS.iter())
            {
                protect(&home.join(tail));
            }
            for dir in all_app_data_dirs(home) {
                protect(&dir);
            }
        }
        let mut projects: Vec<(PathBuf, PathBuf, DenyList)> = ctx
            .projects
            .iter()
            .map(|p| (p.root.clone(), p.canonical.clone(), p.deny.clone()))
            .collect();
        // The session's worktree is judged with its project's own patterns.
        if let Some((real, owner)) = ctx.worktree_owner() {
            projects.push((
                PathBuf::from(ctx.worktree.map_or("", |w| w.path)),
                real,
                owner.deny.clone(),
            ));
        }
        Self {
            projects,
            // A file outside every project also answers to the profile and password store file names.
            default: DenyList::outside_project(),
            protected,
            home: ctx.home.clone(),
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
        self.denial(path).is_some()
    }

    /// Which list refuses `path` (absolute, written or canonical), if any.
    fn denial(&self, path: &Path) -> Option<&'static str> {
        if self.list_for(path).is_denied_rel(&lossy(path)) {
            return Some(rule::DENY_LIST);
        }
        let parts = lower_components(path);
        self.protected
            .iter()
            .any(|prefix| parts.starts_with(prefix))
            .then_some(rule::PROTECTED_DIR)
    }

    /// Whether a file with several names is one of the credential files (the same inode as a
    /// file under the credential folders), or may be, because the index of those ran out of
    /// time: then every such file outside the device's projects is refused.
    fn hard_link_denied(&self, meta: &Metadata, inside_selected_project: bool) -> bool {
        if link_count(meta) <= 1 {
            return false;
        }
        let Some(home) = &self.home else {
            return false;
        };
        let index = secret_index(home);
        file_id(meta).is_some_and(|id| index.ids.contains(&id))
            || (!index.complete && !inside_selected_project)
    }
}

/// The credential file identities under `home`, kept a minute: the walk is bounded but not free.
fn secret_index(home: &Path) -> Arc<SecretIndex> {
    type Kept = Option<(PathBuf, Instant, Arc<SecretIndex>)>;
    static INDEX: LazyLock<Mutex<Kept>> = LazyLock::new(|| Mutex::new(None));
    const TTL: Duration = Duration::from_secs(60);
    const BUDGET: Duration = Duration::from_millis(400);
    let mut slot = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((owner, at, index)) = slot.as_ref() {
        if owner == home && at.elapsed() < TTL {
            return index.clone();
        }
    }
    let paths: Vec<&str> = INDEXED_HOME_PATHS
        .iter()
        .chain(MORE_HOME_CREDENTIALS.iter())
        .copied()
        .collect();
    let index = Arc::new(SecretIndex::build_for(home, &paths, Instant::now(), BUDGET));
    *slot = Some((home.to_path_buf(), Instant::now(), index.clone()));
    index
}

/// Rule 1, step one: whether any mention lexically names the requested path (the same absolute
/// path among their candidates). Nothing here touches the disk. Returns the mentions that do.
fn lexical_hits<'e>(ctx: &Context, evidence: &'e Evidence, requested: &str) -> Vec<&'e String> {
    let wanted: HashSet<String> = link_candidates(requested, ctx.cwd, ctx.roots)
        .into_iter()
        .filter(|path| path.is_absolute() && !is_remote(path))
        .map(|path| path_key(&lossy(&path)))
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    evidence
        .assistant
        .iter()
        .filter(|mention| {
            link_candidates(mention, ctx.cwd, ctx.roots)
                .into_iter()
                .any(|path| wanted.contains(&path_key(&lossy(&path))))
        })
        .collect()
}

/// A link resolved like the desktop's click, as a path (absolute and local), without symlinks.
fn resolved(ctx: &Context, text: &str) -> Option<PathBuf> {
    let (path, _) = resolve_link(text, ctx.cwd, ctx.roots);
    let path = PathBuf::from(path);
    (path.is_absolute() && !is_remote(&path)).then_some(path)
}

/// The requested path as written. A plain relative path is resolved like the desktop's click
/// (the first root that has it, see [`resolved`]). Any other spelling (absolute, `~`, `file://`)
/// is taken literally: `~` expanded, lexically normalized, never re-rooted among the roots, so a
/// missing absolute path stays where it was written and cannot be served as another root's file.
fn literal(ctx: &Context, text: &str) -> Option<PathBuf> {
    if written_relative(text).is_some() {
        return resolved(ctx, text);
    }
    let (path, _) = file_location(text, ctx.cwd);
    let path = PathBuf::from(path);
    (path.is_absolute() && !is_remote(&path)).then_some(path)
}

/// Whether a tool row of the conversation names `canonical`: some token of the tool evidence is,
/// or leads through links to, that same file. `written` is the requested path as written: a tool
/// row that names it (a link) counts when the disk says it leads to `canonical`.
fn tool_evidence(ctx: &Context, evidence: &Evidence, canonical: &Path, written: &Path) -> bool {
    let wanted = path_key(&lossy(canonical));
    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase());
    let mut disk_checks = 0;
    for token in evidence.tool_tokens() {
        for candidate in link_candidates(token, ctx.cwd, ctx.roots) {
            if !candidate.is_absolute() || is_remote(&candidate) {
                continue;
            }
            if path_key(&lossy(&candidate)) == wanted {
                return true;
            }
            // Written through a link or another spelling: the same name, then the disk's word.
            let same_name = candidate
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                == name;
            let is_written = path_key(&lossy(&candidate)) == path_key(&lossy(written));
            if (same_name || is_written) && disk_checks < MAX_TOOL_DISK_CHECKS {
                disk_checks += 1;
                if candidate.canonicalize().is_ok_and(|real| real == canonical) {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether `canonical` lies in a project the device selected.
fn in_selected_project(ctx: &Context, canonical: &Path) -> bool {
    ctx.projects
        .iter()
        .any(|p| p.selected && canonical.starts_with(&p.canonical))
}

/// Whether `tree` (canonical) may be a tree of the freshness rule: not a filesystem root, and
/// neither the home folder nor an ancestor of it (those hold everything the user has).
fn usable_tree(tree: &Path, home: &Path) -> bool {
    let real_home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    tree.parent().is_some() && !home.starts_with(tree) && !real_home.starts_with(tree)
}

/// The one folder the freshness rule trusts: the session's own (its live worktree, else its
/// working folder), for a session and never for a Mono, when it is local, exists, is no
/// filesystem root and neither the home folder nor an ancestor of it, and does not lie in (nor
/// is a worktree of) a known project the device did not select. None at all when the home
/// folder is unknown. A project the device selected needs no freshness: rule 2 passes there.
fn fresh_tree(ctx: &Context) -> Option<PathBuf> {
    let home = ctx.home.as_ref()?;
    if ctx.mono {
        return None;
    }
    let tree = if ctx.worktree.is_some() {
        // A worktree is trusted only through its known project.
        let (worktree, owner) = ctx.worktree_owner()?;
        owner.selected.then_some(worktree)?
    } else {
        let cwd = Path::new(ctx.cwd);
        if ctx.cwd.is_empty() || !cwd.is_absolute() || is_remote(cwd) {
            return None;
        }
        cwd.canonicalize().ok()?
    };
    if !usable_tree(&tree, home) {
        return None;
    }
    // A plain folder inside a project is that project's: it must be the device's.
    let owner = ctx
        .projects
        .iter()
        .filter(|p| tree.starts_with(&p.canonical))
        .max_by_key(|p| p.canonical.as_os_str().len());
    if owner.is_some_and(|p| !p.selected) {
        return None;
    }
    Some(tree)
}

/// How far ahead of now a modification time may be before it is a forgery, not a clock skew.
const FUTURE_SLACK: Duration = Duration::from_secs(60);

/// Whether the file's metadata says it changed at or after `started`. On unix the inode change
/// time decides (a process cannot set it, unlike the modification time, which a command can
/// backdate or push forward); elsewhere the modification time. A modification time ahead of now
/// never counts.
fn changed_since(meta: &Metadata, started: SystemTime) -> bool {
    let now = SystemTime::now();
    let Ok(modified) = meta.modified() else {
        return false;
    };
    if modified > now + FUTURE_SLACK {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(secs) = u64::try_from(meta.ctime()) else {
            return false;
        };
        let changed = SystemTime::UNIX_EPOCH + Duration::new(secs, meta.ctime_nsec() as u32);
        changed >= started
    }
    #[cfg(not(unix))]
    {
        modified >= started
    }
}

/// Rule 2.3: the file changed after the conversation started, lies inside [`fresh_tree`], and
/// in no known project the device did not select (the folder may be an ancestor of one).
fn fresh_in_tree(ctx: &Context, evidence: &Evidence, canonical: &Path) -> bool {
    let Some(started) = ctx.started_at(evidence) else {
        return false;
    };
    let Some(tree) = fresh_tree(ctx) else {
        return false;
    };
    canonical.starts_with(&tree)
        && !ctx
            .projects
            .iter()
            .any(|p| !p.selected && canonical.starts_with(&p.canonical))
        && canonical
            .metadata()
            .is_ok_and(|meta| changed_since(&meta, started))
}

/// Rules 1 to 3 for a requested path: the canonical file it names, when an assistant message
/// mentions exactly that file (and, outside the device's projects, a tool row names it too or it
/// is fresh in the session's folder) and the deny-list lets it through.
fn authorize(
    ctx: &Context,
    guard: &Guard,
    evidence: &Evidence,
    requested: &str,
) -> Result<PathBuf, Refusal> {
    // 1. The lexical mention check, before the disk is touched.
    let hits = lexical_hits(ctx, evidence, requested);
    if hits.is_empty() {
        return Err(Refusal::new(forbidden(), rule::NOT_MENTIONED));
    }
    // 3. The written path against the deny-list, still without the disk.
    let written =
        literal(ctx, requested).ok_or_else(|| Refusal::new(forbidden(), rule::NOT_MENTIONED))?;
    if let Some(by) = guard.denial(&written) {
        return Err(Refusal::new(denied(), by));
    }
    // The file the desktop's click would open, with symlinks resolved. A path that does not
    // exist is `not_found` only when the single mentioned file it names is itself missing and
    // would have been authorized; otherwise it is `forbidden`, like a file that exists but is
    // not authorized, so the answer never tells whether an unauthorized name exists.
    let Ok(canonical) = written.canonicalize() else {
        return Err(if missing_but_authorized(ctx, evidence, &written, &hits) {
            Refusal::new(not_found(), rule::NOT_FOUND)
        } else {
            Refusal::new(forbidden(), rule::NOT_EXACT)
        });
    };
    // 1. Exactly one of the mentioned files: a different spelling of the same name under
    //    another root is a different file.
    let exact = hits.iter().any(|mention| {
        resolved(ctx, mention)
            .is_some_and(|path| path.canonicalize().is_ok_and(|real| real == canonical))
    });
    if !exact {
        return Err(Refusal::new(forbidden(), rule::NOT_EXACT));
    }
    // 2. Outside the device's projects the conversation must also show the file was worked on.
    if !in_selected_project(ctx, &canonical)
        && !tool_evidence(ctx, evidence, &canonical, &written)
        && !fresh_in_tree(ctx, evidence, &canonical)
    {
        return Err(Refusal::new(forbidden(), rule::NO_EVIDENCE));
    }
    // 3. Authorized: now the deny-list on what the link really leads to.
    authorize_parts(ctx, guard, &canonical)?;
    Ok(canonical)
}

/// The deny-lists on the canonical target of an authorized request: the app's folders and the
/// credential places, the owning project's patterns and the session worktree's.
fn authorize_parts(ctx: &Context, guard: &Guard, canonical: &Path) -> Result<(), Refusal> {
    if let Some(by) = guard.denial(canonical) {
        return Err(Refusal::new(denied(), by));
    }
    if let Some(project) = ctx
        .projects
        .iter()
        .find(|p| canonical.starts_with(&p.canonical))
    {
        project
            .deny
            .check(&project.canonical, canonical)
            .map_err(|_| Refusal::new(denied(), rule::DENY_LIST))?;
    }
    // A file of the session's worktree answers to its project's patterns too.
    if let Some((worktree, owner)) = ctx.worktree_owner() {
        if canonical.starts_with(&worktree) {
            owner
                .deny
                .check(&worktree, canonical)
                .map_err(|_| Refusal::new(denied(), rule::DENY_LIST))?;
        }
    }
    Ok(())
}

/// Whether `written` (which does not exist) is itself the file one of the `hits` mentions, and
/// the rest of the rules (a selected project, or a tool row naming it) would have let it through.
/// The names are compared lexically; the disk is only asked for the nearest existing ancestor.
fn missing_but_authorized(
    ctx: &Context,
    evidence: &Evidence,
    written: &Path,
    hits: &[&String],
) -> bool {
    let wanted = path_key(&lossy(written));
    let mentioned = hits
        .iter()
        .any(|mention| resolved(ctx, mention).is_some_and(|p| path_key(&lossy(&p)) == wanted));
    if !mentioned {
        return false;
    }
    inside_by_ancestor(ctx, written) || tool_names_missing(ctx, evidence, written)
}

/// Whether some tool token spells `written` (compared lexically).
fn tool_names_missing(ctx: &Context, evidence: &Evidence, written: &Path) -> bool {
    let wanted = path_key(&lossy(written));
    evidence.tool_tokens().any(|token| {
        link_candidates(token, ctx.cwd, ctx.roots)
            .into_iter()
            .any(|path| path_key(&lossy(&path)) == wanted)
    })
}

/// Whether the missing `written` would lie in a selected project: its nearest existing ancestor,
/// with symlinks resolved, is inside the project's canonical root (a link in the way cannot
/// carry it out of the project by spelling).
fn inside_by_ancestor(ctx: &Context, written: &Path) -> bool {
    let Some(real) = written
        .ancestors()
        .skip(1)
        .find_map(|dir| dir.canonicalize().ok())
    else {
        return false;
    };
    in_selected_project(ctx, &real)
}

/// Whether the evidence gathered so far is enough to decide the call, so a long history need not
/// be read further: an assistant message mentions the requested path (lexically) and either the
/// path lies in a project the device selected, or a tool row names it, or it changed since the
/// conversation started inside the session's own folder. A call that stays undecided is decided
/// by the whole history. Like the call itself it looks at the disk, for the requested file only
/// (its canonical path and its times), and only once a mention names it.
pub fn sufficient(ctx: &Context, params: &Params) -> bool {
    Sufficiency::new(ctx, params).check(ctx, &ctx.evidence())
}

/// [`sufficient`] for a history read page by page: the requested path's candidates are worked
/// out once, and each check looks only at the mentions and tool paths added since the last one
/// (the lists of [`Evidence`] only grow), so the whole scan costs no more than one pass. A
/// mention counts only when it is the requested file itself (the exact check of [`authorize`]),
/// so a first lexical hit that names another file does not end the scan.
pub struct Sufficiency {
    path: String,
    wanted: HashSet<String>,
    assistant_seen: usize,
    tool_seen: usize,
    loose_seen: usize,
    mentioned: bool,
    tooled: bool,
    /// The requested path as written and what it canonicalizes to (`None` when missing), worked
    /// out at the first lexical hit, never before.
    target: Option<Option<(PathBuf, Option<PathBuf>)>>,
}

impl Sufficiency {
    pub fn new(ctx: &Context, params: &Params) -> Self {
        let candidates: Vec<PathBuf> = link_candidates(&params.path, ctx.cwd, ctx.roots)
            .into_iter()
            .filter(|path| path.is_absolute() && !is_remote(path))
            .collect();
        Self {
            path: params.path.clone(),
            wanted: candidates
                .iter()
                .map(|path| path_key(&lossy(path)))
                .collect(),
            assistant_seen: 0,
            tool_seen: 0,
            loose_seen: 0,
            mentioned: false,
            tooled: false,
            target: None,
        }
    }

    /// Whether `evidence` (the same, grown, as at the last call) is enough.
    pub fn check(&mut self, ctx: &Context, evidence: &Evidence) -> bool {
        let names_it = |wanted: &HashSet<String>, text: &String| {
            link_candidates(text, ctx.cwd, ctx.roots)
                .into_iter()
                .any(|path| wanted.contains(&path_key(&lossy(&path))))
        };
        if !self.mentioned && !self.wanted.is_empty() {
            let from = self.assistant_seen.min(evidence.assistant.len());
            let hits: Vec<&String> = evidence.assistant[from..]
                .iter()
                .filter(|text| names_it(&self.wanted, text))
                .collect();
            if !hits.is_empty() {
                let path = self.path.clone();
                let target = self.target.get_or_insert_with(|| {
                    let written = literal(ctx, &path)?;
                    let canonical = written.canonicalize().ok();
                    Some((written, canonical))
                });
                self.mentioned = target.as_ref().is_some_and(|(written, canonical)| {
                    hits.iter().any(|mention| {
                        resolved(ctx, mention).is_some_and(|found| match canonical {
                            Some(real) => found.canonicalize().is_ok_and(|f| f == *real),
                            None => path_key(&lossy(&found)) == path_key(&lossy(written)),
                        })
                    })
                });
            }
        }
        self.assistant_seen = evidence.assistant.len();
        if !self.mentioned {
            return false;
        }
        let inside = match &self.target {
            Some(Some((_, Some(real)))) => {
                in_selected_project(ctx, real) || fresh_in_tree(ctx, evidence, real)
            }
            Some(Some((written, None))) => inside_by_ancestor(ctx, written),
            _ => false,
        };
        if inside {
            return true;
        }
        if !self.tooled {
            let from = self.tool_seen.min(evidence.tool.len());
            let from_loose = self.loose_seen.min(evidence.loose.len());
            let named = evidence.tool[from..]
                .iter()
                .chain(&evidence.loose[from_loose..])
                .any(|text| names_it(&self.wanted, text));
            if named {
                // A lexical hit is final only when the real rule agrees.
                if let Some(Some((written, canonical))) = &self.target {
                    self.tooled = match canonical {
                        Some(real) => tool_evidence(ctx, evidence, real, written),
                        None => tool_names_missing(ctx, evidence, written),
                    };
                }
            }
        }
        self.tool_seen = evidence.tool.len();
        self.loose_seen = evidence.loose.len();
        self.tooled
    }
}

/// Answers one call as no device in particular (tests, tools). See [`serve_as`].
pub fn serve(params: &Params, ctx: &Context) -> Result<Reply, Failure> {
    serve_as("", params, ctx)
}

/// What every step of one call shares.
struct Env<'a> {
    device: &'a str,
    ctx: &'a Context<'a>,
    guard: &'a Guard,
    page: usize,
}

/// Opens `path` (already canonical) for reading, and returns what the open handle says. The open
/// does not follow a link and does not block (a FIFO swapped in after the check would otherwise
/// hang a worker), and the handle must be the very file that was checked: a regular file with
/// the same device and inode.
fn open_regular(path: &Path) -> Result<(File, Metadata), Failure> {
    let checked = std::fs::metadata(path).map_err(|_| not_found())?;
    if !checked.is_file() {
        return Err(forbidden());
    }
    let file = open_plain(path).map_err(|_| not_found())?;
    let opened = file.metadata().map_err(|_| not_found())?;
    if !opened.is_file() || file_id(&opened) != file_id(&checked) {
        return Err(forbidden());
    }
    Ok((file, opened))
}

#[cfg(unix)]
fn open_plain(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
}

// Windows has no FIFO to block on; the handle's metadata is checked by the caller.
#[cfg(not(unix))]
fn open_plain(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

/// Answers one call for `device`. See the module docs for the rules, in the order they are
/// applied.
pub fn serve_as(device: &str, params: &Params, ctx: &Context) -> Result<Reply, Failure> {
    serve_ruled(device, params, ctx).map_err(|refusal| refusal.failure)
}

/// [`serve_as`], with the rule that refused a refused call, for the audit line.
pub fn serve_ruled(device: &str, params: &Params, ctx: &Context) -> Result<Reply, Refusal> {
    let guard = Guard::new(ctx);
    let evidence = ctx.evidence();
    let canonical = authorize(ctx, &guard, &evidence, &params.path)?;
    let env = Env {
        device,
        ctx,
        guard: &guard,
        page: ctx.page_bytes.max(MIN_PAGE_BYTES),
    };
    // 4. Never a directory, nor a device or pipe; opened without blocking and without links.
    let (mut file, metadata) = open_regular(&canonical).map_err(not_regular_or_found)?;
    if guard.hard_link_denied(&metadata, in_selected_project(ctx, &canonical)) {
        return Err(Refusal::new(denied(), rule::HARD_LINK));
    }
    match &params.asset {
        None => {
            serve_file(&env, &mut file, &canonical, &metadata, params).map_err(Refusal::of_code)
        }
        Some(asset) => serve_asset(&env, &mut file, &canonical, &metadata, asset, params),
    }
}

/// The refusal of [`open_regular`]: a missing file, or one that is not a regular file.
fn not_regular_or_found(failure: Failure) -> Refusal {
    let rule = if super::policy::code_of_failure(&failure) == Some(Code::NotFound) {
        rule::NOT_FOUND
    } else {
        rule::NOT_REGULAR
    };
    Refusal::new(failure, rule)
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
    env: &Env,
    file: &mut File,
    canonical: &Path,
    metadata: &Metadata,
    params: &Params,
) -> Result<Reply, Failure> {
    let ctx = env.ctx;
    let size = metadata.len();
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
    // The raw text of a page that would be rendered, for a jump to a line.
    let kind = match kind {
        Kind::Html | Kind::Markdown if params.source => Kind::Text,
        other => other,
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
        Kind::Text | Kind::Markdown => text_page(
            file,
            &mut reply,
            params.offset,
            env.page.min(MENTIONED_FILE_UTF8_PAGE_BYTES),
        )?,
        Kind::Image => {
            reply.mime = html::extension(&reply.name).and_then(|e| mime_of(&e));
            image_page(file, &mut reply, params.offset, env.page)?;
        }
        Kind::Html => {
            let document = html_document(env, file, canonical, metadata)?;
            reply.assets = Some(document.assets.clone());
            string_page(
                &mut reply,
                &document.html,
                params.offset,
                env.page.min(MENTIONED_FILE_UTF8_PAGE_BYTES),
            );
        }
        Kind::Binary => {}
    }
    Ok(reply)
}

/// The size of `text` once JSON-escaped as a string (with its quotes).
fn json_len(text: &str) -> usize {
    let mut len = 2;
    for c in text.chars() {
        len += match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            c if (c as u32) < 0x20 => 6,
            c => c.len_utf8(),
        };
    }
    len
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
    // Invalid bytes become three-byte characters and control characters grow six times when
    // escaped: shrink the page until the reply fits the transport.
    let data = loop {
        let data = String::from_utf8_lossy(&buffer).into_owned();
        if json_len(&data) <= MAX_DATA_JSON_BYTES || buffer.len() <= MIN_PAGE_BYTES {
            break data;
        }
        buffer.truncate(buffer.len() / 2);
        buffer.truncate(complete_prefix(&buffer));
    };
    reply.offset = start;
    reply.served = buffer.len() as u64;
    reply.data = data;
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
/// boundaries and small enough to fit a reply once escaped.
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
    while end - start > MIN_PAGE_BYTES && json_len(&text[start..end]) > MAX_DATA_JSON_BYTES {
        end = start + (end - start) / 2;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
    }
    reply.total = total as u64;
    reply.offset = start as u64;
    reply.data = text[start..end].to_owned();
    reply.served = (end - start) as u64;
    reply.next_offset = (end < total && end > start).then_some(end as u64);
}

/// What a mentioned HTML or markdown file turns into: the web-safe document (HTML only) and the
/// relative image paths it references. The reference set is what an `asset` request is held to.
struct Doc {
    html: String,
    assets: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct DocKey {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

impl DocKey {
    fn of(path: &Path, meta: &Metadata) -> Self {
        Self {
            path: path.to_path_buf(),
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }
}

/// The few documents last built, so a page or an asset request does not transform again.
const DOC_CACHE_ENTRIES: usize = 4;

type KeptDocs = VecDeque<(DocKey, Arc<Doc>)>;

static DOCS: LazyLock<Mutex<KeptDocs>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

fn cached_doc(key: &DocKey) -> Option<Arc<Doc>> {
    let docs = DOCS.lock().unwrap_or_else(|e| e.into_inner());
    docs.iter()
        .find(|(known, _)| known == key)
        .map(|(_, doc)| doc.clone())
}

fn keep_doc(key: DocKey, doc: Arc<Doc>) {
    let mut docs = DOCS.lock().unwrap_or_else(|e| e.into_inner());
    docs.retain(|(known, _)| *known != key);
    if docs.len() >= DOC_CACHE_ENTRIES {
        docs.pop_front();
    }
    docs.push_back((key, doc));
}

/// Transforms in flight, so a device cannot start a second while one runs, and no more than a
/// few run at all.
static FLIGHT: LazyLock<Mutex<FlightState>> = LazyLock::new(|| Mutex::new(FlightState::default()));
const MAX_TRANSFORMS: usize = 4;

#[derive(Default)]
struct FlightState {
    devices: HashSet<String>,
    /// Transforms running, those of an unnamed device included.
    total: usize,
}

impl FlightState {
    /// Admits one more transform for `device`, or says it is busy.
    fn admit(&mut self, device: &str, max: usize) -> bool {
        if self.total >= max || (!device.is_empty() && self.devices.contains(device)) {
            return false;
        }
        self.total += 1;
        if !device.is_empty() {
            self.devices.insert(device.to_owned());
        }
        true
    }
}

struct Flight(Option<String>);

impl Flight {
    /// `Busy` when `device` already has a transform running (an unnamed device is not limited
    /// per device, only by the global cap).
    fn enter(device: &str) -> Result<Self, Failure> {
        let mut running = FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        // Tests run many transforms at once with no device; the cap itself is tested on `admit`.
        let max = if cfg!(test) { 4096 } else { MAX_TRANSFORMS };
        if !running.admit(device, max) {
            return Err(Code::Busy.failure());
        }
        Ok(Self((!device.is_empty()).then(|| device.to_owned())))
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        let mut running = FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        running.total = running.total.saturating_sub(1);
        if let Some(device) = self.0.take() {
            running.devices.remove(&device);
        }
    }
}

/// The sanitized document of a mentioned HTML file, from the cache or built now (one build at a
/// time per device, bounded in source size, work and output size).
fn html_document(
    env: &Env,
    file: &mut File,
    canonical: &Path,
    metadata: &Metadata,
) -> Result<Arc<Doc>, Failure> {
    let key = DocKey::of(canonical, metadata);
    if let Some(doc) = cached_doc(&key) {
        return Ok(doc);
    }
    if metadata.len() > MAX_REFERENCING_SOURCE_BYTES {
        return Err(Code::TooLarge.failure());
    }
    let _flight = Flight::enter(env.device)?;
    if let Some(doc) = cached_doc(&key) {
        return Ok(doc);
    }
    let raw = read_at(file, 0, metadata.len() as usize)?;
    let source = String::from_utf8_lossy(&raw).into_owned();
    // The true shape of the tree the parser builds (clones of formatting elements included):
    // a document with too many elements, or nested too deep, is refused before any transform.
    let shape = prepass::scan(&source).map_err(|_| sandbox::too_complex())?;
    // The sheets are read here, with the folder and deny-list checks, in the order the inliner
    // asks for them (sorted, once each); the transform gets their text and opens no file.
    let folder = canonical.parent().unwrap_or(canonical).to_path_buf();
    let folder_sheets = FolderSheets {
        folder,
        guard: env.guard.clone(),
        used: AtomicU64::new(0),
    };
    let mut hrefs = shape.stylesheets.clone();
    hrefs.sort_unstable();
    hrefs.dedup();
    // `a.css`, `./a.css` and `././a.css` are one file: it is read, counted and budgeted once, and
    // every href that names it keeps its own key for the inliner.
    let mut loaded: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut sheets: Vec<(String, String)> = Vec::new();
    for href in hrefs {
        let Some(canonical) = folder_sheets.resolve(&href) else {
            continue;
        };
        if !loaded.contains_key(&canonical) {
            if loaded.len() >= sandbox::MAX_SHEETS {
                return Err(Code::TooLarge.failure());
            }
            let text = folder_sheets.read(&canonical);
            loaded.insert(canonical.clone(), text);
        }
        if let Some(Some(text)) = loaded.get(&canonical) {
            sheets.push((href, text.clone()));
        }
    }
    // The style the inliner parses: every `<style>`, then the sheets, joined with newlines.
    let css: String = shape
        .styles
        .iter()
        .map(String::as_str)
        .chain(sheets.iter().map(|(_, text)| text.as_str()))
        .flat_map(|part| [part, "\n"])
        .collect();
    let census = cost::Census::from_shape(&shape);
    let work = cost::assess(&css, &census);
    if census.elements.saturating_mul(css.len() as u64) > MAX_SCAN_WORK
        || work.copy > MAX_COPY_WORK
        || work.matching > MAX_MATCH_WORK
    {
        return Err(Code::TooLarge.failure());
    }
    let document = sandbox::transformer().run(sandbox::Input {
        html: source,
        sheets,
    })?;
    if document.html.len() as u64 > MENTIONED_FILE_MAX_BYTES {
        return Err(Code::TooLarge.failure());
    }
    let doc = Arc::new(Doc {
        html: document.html,
        assets: document.assets,
    });
    keep_doc(key, doc.clone());
    Ok(doc)
}

/// The image keys a mentioned markdown file references (same rules as the html: [`html::normalize_reference`]).
fn markdown_assets(source: &str) -> Vec<String> {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    let mut assets: Vec<String> = Vec::new();
    for event in Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    ) {
        if let Event::Start(Tag::Image { dest_url, .. }) = event {
            if let Some(key) = html::normalize_reference(&dest_url) {
                if !assets.contains(&key) {
                    assets.push(key);
                }
            }
        }
    }
    assets
}

/// What the mentioned file `canonical` references, from the cache or by reading it.
fn references(
    env: &Env,
    file: &mut File,
    canonical: &Path,
    metadata: &Metadata,
) -> Result<Arc<Doc>, Failure> {
    match kind_by_extension(&file_name(canonical)) {
        Some(Kind::Html) => html_document(env, file, canonical, metadata),
        Some(Kind::Markdown) => {
            let key = DocKey::of(canonical, metadata);
            if let Some(doc) = cached_doc(&key) {
                return Ok(doc);
            }
            if metadata.len() > MAX_REFERENCING_SOURCE_BYTES {
                return Err(Code::TooLarge.failure());
            }
            let raw = read_at(file, 0, metadata.len() as usize)?;
            let doc = Arc::new(Doc {
                html: String::new(),
                assets: markdown_assets(&String::from_utf8_lossy(&raw)),
            });
            keep_doc(key, doc.clone());
            Ok(doc)
        }
        _ => Err(forbidden()),
    }
}

/// Rule 6: an image the mentioned HTML or markdown file references, inside the root of its project (else its folder).
fn serve_asset(
    env: &Env,
    parent_file: &mut File,
    parent: &Path,
    parent_meta: &Metadata,
    asset: &str,
    params: &Params,
) -> Result<Reply, Refusal> {
    let ctx = env.ctx;
    let (mut file, metadata, canonical, mime) =
        resolve_asset(env, parent_file, parent, parent_meta, asset)?;
    let mut reply = Reply {
        path: lossy(&canonical),
        name: file_name(&canonical),
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
    image_page(&mut file, &mut reply, params.offset, env.page)?;
    Ok(reply)
}

/// The checks of rule 6, ending at the opened image: its handle, metadata, canonical path and
/// image mime. Shared by the paged reply and the encrypted transfer.
fn resolve_asset(
    env: &Env,
    parent_file: &mut File,
    parent: &Path,
    parent_meta: &Metadata,
    asset: &str,
) -> Result<(File, Metadata, PathBuf, &'static str), Refusal> {
    let (ctx, guard) = (env.ctx, env.guard);
    // Only a page that shows images has assets.
    if !matches!(
        kind_by_extension(&file_name(parent)),
        Some(Kind::Html | Kind::Markdown)
    ) {
        return Err(forbidden().into());
    }
    let lower = asset.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "%00"]
        .iter()
        .any(|bad| lower.contains(bad))
    {
        return Err(forbidden().into());
    }
    let key = html::normalize_key(asset).ok_or_else(forbidden)?;
    // The file must reference it: otherwise the folder's images could be probed one by one.
    // Decided from the mentioned file alone, before the asset's path is looked at.
    let referenced = references(env, parent_file, parent, parent_meta)?;
    if !referenced.assets.contains(&key) {
        return Err(forbidden().into());
    }
    let folder = parent.parent().ok_or_else(forbidden)?;
    // The root the document was authorized under: the selected project holding it, else its own
    // folder. An image may sit anywhere inside that root (`../img/a.png`, an absolute path or a
    // `file://` URL included), never outside it.
    let root = asset_root(ctx, parent, folder);
    let joined = if key.starts_with('/') {
        PathBuf::from(&key)
    } else {
        folder.join(&key)
    };
    // Lexically first, so a path outside the root is refused without touching the disk (no
    // probing of what exists elsewhere), then again after the links are followed.
    if !lexical_clean(&joined).starts_with(&root) {
        return Err(forbidden().into());
    }
    if let Some(by) = guard.denial(&joined) {
        return Err(Refusal::new(denied(), by));
    }
    // A `..` after a link can leave the root only once the link is followed: the parent folder is
    // resolved and checked first, so a missing target outside the root answers like any other
    // refusal and never says whether it exists.
    if key.split('/').any(|part| part == "..") {
        let parent_dir = joined.parent().ok_or_else(forbidden)?;
        let resolved = parent_dir.canonicalize().map_err(|_| forbidden())?;
        if !resolved.starts_with(&root) {
            return Err(forbidden().into());
        }
    }
    let canonical = joined.canonicalize().map_err(|_| not_found())?;
    if !canonical.starts_with(&root) {
        return Err(forbidden().into());
    }
    if let Some(by) = guard.denial(&canonical) {
        return Err(Refusal::new(denied(), by));
    }
    let (file, metadata) = open_regular(&canonical).map_err(not_regular_or_found)?;
    if guard.hard_link_denied(&metadata, in_selected_project(ctx, &canonical)) {
        return Err(Refusal::new(denied(), rule::HARD_LINK));
    }
    let name = file_name(&canonical);
    // The extension of the canonical target decides, so a link named `a.png` cannot serve a
    // different kind of file.
    let mime = html::extension(&name)
        .and_then(|e| mime_of(&e))
        .ok_or_else(forbidden)?;
    if metadata.len() > ctx.max_bytes {
        return Err(Code::TooLarge.failure().into());
    }
    Ok((file, metadata, canonical, mime))
}

/// The folder an asset of the document `parent` must stay inside: the selected project that
/// holds the document (the deepest one), else the document's own folder.
fn asset_root(ctx: &Context, parent: &Path, folder: &Path) -> PathBuf {
    ctx.projects
        .iter()
        .filter(|p| p.selected && parent.starts_with(&p.canonical))
        .max_by_key(|p| p.canonical.as_os_str().len())
        .map_or_else(|| folder.to_path_buf(), |p| p.canonical.clone())
}

/// `path` with its `.` and `..` parts resolved by name only (no disk access).
fn lexical_clean(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The linked stylesheets of a mentioned HTML file: local, inside its folder, not denied.
struct FolderSheets {
    folder: PathBuf,
    guard: Guard,
    used: AtomicU64,
}

impl FolderSheets {
    /// The canonical path of the stylesheet `href` names, when it may be read.
    fn resolve(&self, href: &str) -> Option<PathBuf> {
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
        Some(canonical)
    }

    fn read(&self, canonical: &Path) -> Option<String> {
        let (mut file, metadata) = open_regular(canonical).ok()?;
        if metadata.len() > MAX_SHEET_BYTES {
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

impl html::Sheets for FolderSheets {
    fn load(&self, href: &str) -> Option<String> {
        self.read(&self.resolve(href)?)
    }
}

// The tests build POSIX paths (`/tmp/...`, `~/`) for the rules that are about them; the ones
// about parameters, kinds, budgets, HTML, assets and extraction run on every OS.
#[cfg(test)]
mod tests;
