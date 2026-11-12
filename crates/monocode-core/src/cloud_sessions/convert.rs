//! Turning a cloud session into an archived LOCAL copy before its codespace is stopped or
//! deleted ([`CloudSessions::convert_session`]), and restoring the code from that copy
//! later ([`restore_code`]).
//!
//! What one conversion keeps, in this order, failing (and creating nothing) if any required
//! step fails:
//!
//! 1. the codespace is woken and reconnected (a user action);
//! 2. the full transcript is read from the host (`sessions.sync`);
//! 3. `.evidence` is pulled ([`CloudSessions::pull_evidence`]);
//! 4. one `sh` script on the codespace stages Claude's own JSONL transcript and the git state
//!    of the workspace: `manifest` facts, a `git bundle` of the commits that are on no
//!    remote ref, and a binary patch of every uncommitted change including untracked files
//!    (built with a temporary `GIT_INDEX_FILE`, so the real index is never touched). The
//!    stage folder is copied with `gh codespace cp` and removed from the codespace;
//! 5. a new local session in the binding's local project is created, filled (title, rows,
//!    agent settings, provider id), archived and tagged with its origin;
//! 6. the JSONL is written, with the codespace path rewritten, to
//!    `<claude dir>/projects/<encoded local cwd>/<provider id>.jsonl` (never overwriting);
//! 7. the git files land in `<db>.threads/<folder>/codespace/` and the evidence in
//!    `.../evidence/`;
//! 8. the cloud record is forgotten, so retrying after a later failure (for example the
//!    stop itself) does not convert twice.
//!
//! Code is bundled to the local disk, never pushed anywhere.
//!
//! # Files-only archive (Docker sandboxes)
//!
//! When the sandbox reconnect reports that the devcontainer was recreated, or that the Host
//! cannot be brought back although the sandbox itself is fine, the session is still archived
//! in a reduced "files only" mode so unpushed work can be saved before a stop or delete:
//!
//! * a recreated container is archived this way directly. A Host that merely could not be
//!   brought back may still hold the conversation, so that case needs the user's
//!   confirmation first ([`CloudSessions::convert_session_confirmed`]);
//! * the conversation is read only if a connected Host still answers (nothing is woken for
//!   it); otherwise the copy has no history, and the warnings say the history may be
//!   incomplete (the store keeps no copy of a remote transcript);
//! * the git snapshot (steps 4 and 5 above) runs against the workspace in the OUTER sandbox,
//!   a bind mount that survives a container recreate, with the same script and the same
//!   verified tar stream, but over `sh -s` in the outer sandbox instead of
//!   `devcontainer exec`;
//! * Claude's JSONL lives in the old container, so it is skipped (a warning says so) and
//!   `.evidence` is not pulled.
//!
//! A sandbox without a devcontainer ([`crate::remote::SandboxMode::Plain`]) has no container
//! to lose: its scripts and copies run in the workspace of the outer sandbox, the session's
//! folder is that workspace, and the files-only mode only applies when its Host cannot be
//! brought back.
//!
//! Only [`CloudSessions::convert_session`] chooses this mode, and it is the user's archive
//! action. Nothing in a background path (polls, Monos, the app API) reaches it.
use super::*;
use crate::codespaces::valid_remote_path;
use crate::remote::SandboxTarget;
use backend::docker::DockerBackend;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Folder inside the local session folder that holds the code state.
pub const CODESPACE_DIR: &str = "codespace";
pub const MANIFEST: &str = "manifest.json";
pub const BUNDLE: &str = "commits.bundle";
pub const PATCH: &str = "changes.patch";
/// The `git format-patch` series of the unpushed commits.
pub const PATCHES: &str = "patches";
/// Claude's transcript exactly as it was on the codespace.
pub const RAW_JSONL: &str = "claude.jsonl";
/// Written by [`restore_code`]; its presence means the action was used.
pub const RESTORED: &str = "restored.json";
/// Session setting recording where a converted session came from.
pub fn origin_key(backend: CloudBackendKind, local_id: i64) -> String {
    match backend {
        CloudBackendKind::Codespaces => format!("codespace-origin:{local_id}"),
        CloudBackendKind::DockerSandbox => format!("sandbox-origin:{local_id}"),
    }
}

/// Session-independent setting listing what deleting a workspace would lose.
pub fn unsaved_key(cloud: &CloudRef) -> String {
    match cloud.kind {
        CloudBackendKind::Codespaces => format!("codespace-unsaved:{}", cloud.name),
        CloudBackendKind::DockerSandbox => format!("sandbox-unsaved:{}", cloud.name),
    }
}

/// Runs on the codespace with `sh -s`. `@WS@` and `@ID@` are substituted (validated first).
/// Prints `key=value` lines; everything it stages lives under the printed `stage`. Errors are
/// not masked: git's own message ends up in stderr and fails the conversion. The stage is
/// removed again unless the script finished.
const SNAPSHOT_SCRIPT: &str = r#"set -eu
ws='@WS@'
id='@ID@'
stage=$(mktemp -d /tmp/monoptah-archive.XXXXXX)
ok=0
trap 'if [ "$ok" != 1 ]; then rm -rf "$stage"; fi' EXIT
echo "stage=$stage"
if [ -n "$id" ]; then
  f=$(find "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects" -maxdepth 2 -type f -name "$id.jsonl" 2>/dev/null | head -n 1 || true)
  if [ -n "$f" ]; then cp "$f" "$stage/claude.jsonl"; echo "jsonl=1"; fi
fi
if [ ! -d "$ws" ] || ! git -C "$ws" rev-parse --git-dir >/dev/null 2>&1; then
  echo "nogit=1"
else
  cd "$ws"
  export GIT_TERMINAL_PROMPT=0
  head=$(git rev-parse -q --verify HEAD || true)
  if [ -z "$head" ]; then
    echo "nohead=1"
  else
    mkdir -p "$stage/codespace"
    fetcher="git"
    if command -v timeout >/dev/null 2>&1; then fetcher="timeout 60 git"; fi
    if $fetcher fetch -q --prune origin >/dev/null 2>&1; then echo "fetch=1"; else echo "fetch=0"; fi
    echo "head=$head"
    echo "branch=$(git symbolic-ref --short -q HEAD || true)"
    echo "remote=$(git remote get-url origin 2>/dev/null || true)"
    up=$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null || true)
    if [ -z "$up" ]; then up=$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true); fi
    if [ -z "$up" ]; then
      for c in origin/main origin/master; do
        if git rev-parse -q --verify "$c" >/dev/null 2>&1; then up=$c; break; fi
      done
    fi
    echo "upstream=$up"
    base=""
    if [ -n "$up" ]; then base=$(git merge-base HEAD "$up" || true); fi
    echo "base=$base"
    onorigin=0
    if [ -n "$base" ] && [ -n "$(git branch -r --list 'origin/*' --contains "$base" | head -n 1)" ]; then onorigin=1; fi
    echo "base_on_origin=$onorigin"
    set -- HEAD --branches
    if git rev-parse -q --verify refs/stash >/dev/null; then set -- "$@" refs/stash; echo "stash=1"; fi
    git for-each-ref --format='ref=%(refname) %(objectname)' refs/heads refs/stash | head -n 200
    n=$(git rev-list --count "$@" --not --remotes=origin)
    echo "unpushed=$n"
    if [ "$n" -gt 0 ]; then
      git bundle create -q "$stage/codespace/commits.bundle" "$@" --not --remotes=origin
      echo "bundle=1"
    fi
    if [ -n "$base" ]; then
      m=$(git rev-list --count "$base..HEAD")
      if [ "$m" -gt 0 ]; then
        git format-patch -q --binary -o "$stage/codespace/patches" "$base..HEAD" >/dev/null
        echo "patches=$m"
      fi
    fi
    idx="$stage/index.tmp"
    real=$(git rev-parse --git-path index)
    if [ -f "$real" ]; then cp "$real" "$idx"; else GIT_INDEX_FILE="$idx" git read-tree HEAD; fi
    GIT_INDEX_FILE="$idx" git add -A
    GIT_INDEX_FILE="$idx" git diff --cached --binary HEAD > "$stage/codespace/changes.patch"
    rm -f "$idx"
    if [ -s "$stage/codespace/changes.patch" ]; then echo "patch=1"; else rm -f "$stage/codespace/changes.patch"; fi
    emit() {
      while IFS= read -r p; do
        kb=$(du -sk -- "${p%/}" 2>/dev/null | cut -f1 || true)
        echo "ignored=${kb:-0} $p"
      done
    }
    regen='(^|/)(node_modules|target|\.venv|venv|__pycache__|dist|build|\.next|\.turbo|\.cache|\.gradle|Pods|vendor/bundle)/'
    all=$(git -c core.quotePath=false ls-files -o -i --exclude-standard --directory | grep -v -e '^\.evidence/$' -e '^CLAUDE\.local\.md$' | while IFS= read -r p; do
      q="${p%/}"
      if [ -L "$q" ] && [ -d "$q" ]; then printf '%s/\n' "$q"; else printf '%s\n' "$p"; fi
    done || true)
    printf '%s\n' "$all" | grep -E "$regen" | head -n 40 | emit
    lost=$(printf '%s\n' "$all" | grep -Ev "$regen" | grep -v '^$' | head -n 41 || true)
    count=$(printf '%s\n' "$lost" | grep -c . || true)
    if [ "${count:-0}" -gt 40 ]; then echo "ignored_truncated=1"; fi
    printf '%s\n' "$lost" | grep -v '^$' | head -n 40 | emit
    echo "size_kb=$(du -sk "$stage" | cut -f1)"
  fi
fi
ok=1
"#;

/// Everything the snapshot script reported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) stage: String,
    pub(super) jsonl: bool,
    pub(super) nogit: bool,
    pub(super) nohead: bool,
    pub(super) fetched: bool,
    pub(super) head: Option<String>,
    pub(super) branch: Option<String>,
    pub(super) remote: Option<String>,
    pub(super) upstream: Option<String>,
    pub(super) base: Option<String>,
    pub(super) base_on_origin: bool,
    pub(super) stash: bool,
    pub(super) refs: Vec<(String, String)>,
    pub(super) unpushed: u64,
    pub(super) bundle: bool,
    pub(super) patches: u64,
    pub(super) patch: bool,
    /// Ignored files and folders that are not saved: size in KiB and path (capped by the script).
    pub(super) ignored: Vec<(u64, String)>,
    /// More non-regenerable ignored entries exist than the script lists.
    pub(super) ignored_truncated: bool,
    pub(super) size_kb: u64,
}

/// Total size of the code state above which a conversion fails instead of copying it.
const SIZE_LIMIT_KB: u64 = 500 * 1024;
/// The stage is always created here; anything else is never removed.
const STAGE_PREFIX: &str = "/tmp/monoptah-archive.";

pub(super) fn parse_snapshot(out: &str) -> Result<Snapshot, String> {
    let mut snap = Snapshot::default();
    for line in out.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let text = || (!value.is_empty()).then(|| value.to_string());
        let flag = value == "1";
        match key {
            "stage" => snap.stage = value.into(),
            "jsonl" => snap.jsonl = flag,
            "nogit" => snap.nogit = flag,
            "nohead" => snap.nohead = flag,
            "fetch" => snap.fetched = flag,
            "head" => snap.head = text(),
            "branch" => snap.branch = text(),
            "remote" => snap.remote = text(),
            "upstream" => snap.upstream = text(),
            "base" => snap.base = text(),
            "base_on_origin" => snap.base_on_origin = flag,
            "stash" => snap.stash = flag,
            "ref" => {
                if let Some((name, sha)) = value.split_once(' ') {
                    snap.refs.push((name.into(), sha.trim().into()));
                }
            }
            "unpushed" => snap.unpushed = value.parse().unwrap_or(0),
            "bundle" => snap.bundle = flag,
            "patches" => snap.patches = value.parse().unwrap_or(0),
            "patch" => snap.patch = flag,
            "ignored" => {
                if let Some((kb, path)) = value.split_once(' ') {
                    snap.ignored.push((kb.parse().unwrap_or(0), path.into()));
                }
            }
            "ignored_truncated" => snap.ignored_truncated = flag,
            "size_kb" => snap.size_kb = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    if !snap.stage.starts_with(STAGE_PREFIX) {
        return Err("The codespace did not report where it staged the files".into());
    }
    valid_remote_path(&snap.stage)?;
    Ok(snap)
}

pub(super) fn valid_hex(text: &str) -> bool {
    (40..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Claude Code's folder name for a working directory: every non-alphanumeric ASCII
/// character becomes `-` (verified against `~/.claude/projects`).
pub fn claude_project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Claude Code shortens (and hashes) folder names above this many characters; we cannot
/// reproduce the hash, so such paths are not converted.
const CLAUDE_NAME_LIMIT: usize = 200;

pub(super) fn default_claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".claude"))
        })
        .unwrap_or_else(|| PathBuf::from(".claude"))
}

/// Replaces every path in `from` (matched on boundaries) by `to` in a JSONL text. `to` is
/// and every `from` are escaped for use inside a JSON string. Longer paths are tried first. A match must not be
/// the tail of a longer path (`/tmp/workspaces/app`) or the head of a longer name
/// (`/workspaces/app-two`, `/workspaces/app.git`).
pub(super) fn rewrite_cwd(text: &str, from: &[&str], to: &str) -> String {
    let escaped = serde_json::to_string(to).unwrap_or_default();
    let to = &escaped[1..escaped.len().saturating_sub(1).max(1)];
    // The text is JSONL, so a path with a quote or backslash is stored escaped.
    let mut froms: Vec<String> = from
        .iter()
        .filter(|f| !f.is_empty())
        .map(|f| {
            let escaped = serde_json::to_string(f).unwrap_or_default();
            escaped[1..escaped.len().saturating_sub(1).max(1)].to_string()
        })
        .collect();
    froms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    froms.dedup();
    let mut text = text.to_string();
    for from in froms.iter().map(String::as_str) {
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        let mut before: Option<char> = None;
        while let Some(at) = rest.find(from) {
            let prefix = &rest[..at];
            let after = &rest[at + from.len()..];
            let left = prefix.chars().next_back().or(before).is_none_or(|c| {
                !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '~'))
            });
            let mut next = after.chars();
            let right = match next.next() {
                None => true,
                Some(c) if c.is_ascii_alphanumeric() || c == '-' || c == '_' => false,
                // A dot ends a sentence unless a name continues after it.
                Some('.') => !next
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                Some(_) => true,
            };
            out.push_str(prefix);
            let replaced = left && right;
            out.push_str(if replaced { to } else { from });
            before = if replaced {
                to.chars().next_back()
            } else {
                from.chars().next_back()
            };
            rest = after;
        }
        out.push_str(rest);
        text = out;
    }
    text
}

/// What happened to Claude's own transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resume {
    /// Written to the local Claude folder: `claude --resume <id>` works from the project.
    Installed(PathBuf),
    /// A file with that id already existed locally with the same entries and was left alone.
    AlreadyThere(PathBuf),
    /// The incoming transcript extends the local file with that id: it replaced it, and the
    /// old file is kept next to it as `<id>.jsonl.pre-cloud`.
    Replaced(PathBuf),
    /// The local file with that id diverged from the incoming one and was left untouched.
    /// The incoming transcript is installed under this new session id.
    Forked(String),
    /// Not available; the local copy starts a fresh Claude conversation.
    Missing(String),
}

/// Internal marker: the sandbox of a moved session was recreated (see `continue_locally`).
pub(super) const SANDBOX_RECREATED: &str = "sandbox recreated";

/// What a conversion is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// Save an archived local copy of a cloud session (Keep, Stop and Delete).
    Archive,
    /// "Continue locally": the session carries on as ONE local session. Nothing is archived.
    /// `moved` is the local session that already lives in the cloud (it keeps its rows and
    /// its id); `None` is a session born in the cloud, which gets a new local session.
    Continue { moved: Option<i64> },
}

impl Mode {
    pub(super) fn continuing(self) -> bool {
        matches!(self, Self::Continue { .. })
    }
    pub(super) fn moved(self) -> Option<i64> {
        match self {
            Self::Continue { moved } => moved,
            Self::Archive => None,
        }
    }
}

/// What a conversion produced.
#[derive(Clone, Debug)]
pub struct Converted {
    /// The cloud session that was converted (now forgotten locally).
    pub cloud_id: i64,
    /// The new archived local session.
    pub local_id: i64,
    /// The new session as the store lists it (archived).
    pub summary: crate::store::Session,
    pub codespace: String,
    pub title: String,
    pub rows: usize,
    pub resume: Resume,
    pub evidence: Vec<PathBuf>,
    /// The `codespace/` folder in the local session folder (manifest, bundle, patches).
    pub code_dir: Option<PathBuf>,
    /// Ignored files and folders that were NOT saved: size in KiB and path.
    pub ignored: Vec<(u64, String)>,
    /// The codespace had more non-regenerable ignored entries than `ignored` lists.
    pub ignored_truncated: bool,
    /// Some evidence files could not be copied (the rest were kept).
    pub evidence_partial: bool,
    /// Problems that did not stop the conversion.
    pub warnings: Vec<String>,
    /// The cloud session had a Claude conversation but its raw transcript was not copied
    /// here (not found, or not saved), so the cloud copy may be the only one left.
    pub transcript_lost: bool,
    /// Whether the conversation itself was saved. `false` for a files-only archive that
    /// could not read the history: only the code state is in the local copy.
    pub history_saved: bool,
}

/// Why [`CloudSessions::convert_session_confirmed`] did not convert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConvertError {
    /// The sandbox is fine but its Host could not be brought back, so the conversation may
    /// still be intact there. Converting files-only would forget the cloud record and the
    /// history with it, so it needs the user's explicit confirmation: call again with
    /// `files_only_confirmed` set. Nothing was changed.
    NeedsFilesOnlyConfirm {
        reason: String,
    },
    Failed(String),
}
impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsFilesOnlyConfirm { reason } => write!(
                f,
                "The conversation of this session could not be reached ({reason}). Retry, or archive only its files."
            ),
            Self::Failed(text) => f.write_str(text),
        }
    }
}
impl From<String> for ConvertError {
    fn from(text: String) -> Self {
        Self::Failed(text)
    }
}
impl From<&str> for ConvertError {
    fn from(text: &str) -> Self {
        Self::Failed(text.into())
    }
}

/// Names a converted session's refs under `refs/codespace/`, so sessions that share a HEAD
/// never share (or remove) each other's pins.
pub fn pin_key(cloud_id: i64) -> String {
    format!("s{}", cloud_id.unsigned_abs())
}

/// Directory names whose content a build or install brings back, so skipping them loses nothing.
const REGENERABLE_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
    ".gradle",
    "Pods",
];

/// Whether an ignored entry reported by the snapshot (`dir/` for folders) is a dependency or
/// build directory that is rebuilt on demand. Matches by path component at any depth.
///
/// Known blind spot, accepted: this is about the directory, so a hand-made or secret file inside
/// such a directory (`dist/.env`, `deploy/build/prod.env`) counts as regenerable and does not
/// warn. It is still listed in the manifest.
pub fn is_regenerable(path: &str) -> bool {
    // Git quotes paths with quotes, backslashes or control characters.
    let path = path
        .strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .unwrap_or(path);
    let is_dir = path.ends_with('/');
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let last = parts.len().saturating_sub(1);
    // The final component only counts when the entry is a directory.
    let dir_parts = |i: usize| i < last || is_dir;
    parts
        .iter()
        .enumerate()
        .any(|(i, part)| dir_parts(i) && REGENERABLE_DIRS.contains(part))
        || parts
            .windows(2)
            .enumerate()
            .any(|(i, w)| w == ["vendor", "bundle"] && dir_parts(i + 1))
}

/// Entries the codespace listed that cannot be rebuilt, so skipping them can lose work.
pub fn unsaved_names(ignored: &[(u64, String)]) -> Vec<&str> {
    ignored
        .iter()
        .filter(|(_, p)| !is_regenerable(p))
        .map(|(_, p)| p.as_str())
        .collect()
}

impl Converted {
    /// Ignored entries that were not saved and cannot be rebuilt.
    pub fn unsaved_names(&self) -> Vec<&str> {
        unsaved_names(&self.ignored)
    }
}

/// What deleting the codespace would lose after the copies in `converted` were saved: ignored
/// files that cannot be rebuilt (first three named) and evidence that was only partly copied.
pub fn unsaved_lines(converted: &[Converted]) -> Vec<String> {
    const NAMED: usize = 3;
    let mut lines = Vec::new();
    let names: Vec<&str> = converted.iter().flat_map(|c| c.unsaved_names()).collect();
    if !names.is_empty() {
        let list = names[..names.len().min(NAMED)].join(", ");
        let more = match names.len().saturating_sub(NAMED) {
            0 => String::new(),
            n => format!(" and {n} more"),
        };
        let cut = if converted.iter().any(|c| c.ignored_truncated) {
            " The codespace has even more."
        } else {
            ""
        };
        lines.push(format!(
            "Ignored files are not part of the local copy: {list}{more}.{cut}"
        ));
    }
    for c in converted.iter().filter(|c| !c.history_saved) {
        lines.push(format!(
            "The conversation of \u{201c}{}\u{201d} was not saved.",
            c.title.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
    for c in converted.iter().filter(|c| c.evidence_partial) {
        lines.push(format!(
            "Some evidence files of \u{201c}{}\u{201d} could not be copied.",
            c.title.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
    lines
}

/// The warning for ignored entries that cannot be rebuilt, or `None` when only regenerable
/// ones were skipped. Names the first few paths. `truncated`: the codespace had more than it
/// listed.
pub(super) fn ignored_warning(ignored: &[(u64, String)], truncated: bool) -> Option<String> {
    const NAMED: usize = 3;
    let lost: Vec<&(u64, String)> = ignored.iter().filter(|(_, p)| !is_regenerable(p)).collect();
    if lost.is_empty() {
        return None;
    }
    let kb: u64 = lost.iter().map(|(kb, _)| kb).sum();
    let size = if kb < 1024 {
        "under 1 MB".to_string()
    } else {
        format!("about {} MB", kb / 1024)
    };
    let list = lost
        .iter()
        .take(NAMED)
        .map(|(_, p)| p.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let tail = match lost.len().saturating_sub(NAMED) {
        0 => String::new(),
        more => format!(" and {more} more"),
    };
    let what = match (lost.len(), truncated) {
        (1, false) => "1 ignored file or folder".to_string(),
        (n, false) => format!("{n} ignored files or folders"),
        (n, true) => format!("At least {n} ignored files or folders"),
    };
    let verb = if lost.len() == 1 && !truncated {
        "was"
    } else {
        "were"
    };
    let more = if truncated {
        " The codespace had more than the manifest lists."
    } else {
        ""
    };
    Some(format!(
        "{what} ({size}) {verb} not saved: {list}{tail}.{more} They are listed in the session's codespace manifest."
    ))
}

pub(super) fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else if meta.is_file() {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

impl CloudSessions {
    /// Tells the sandbox backend where the container of `cloud` is (from the paired machine,
    /// no ssh). A no-op for a codespace.
    pub(super) fn prime_backend(&self, cloud: &CloudRef) {
        if cloud.kind != CloudBackendKind::DockerSandbox {
            return;
        }
        let Some(deps) = &self.sandbox else { return };
        if let Some(target) = self.host.machines().ok().and_then(|machines| {
            machines
                .into_iter()
                .filter_map(|m| m.sandbox)
                .find(|s| s.name == cloud.name)
        }) {
            deps.backend.remember_target(&target);
        }
    }

    /// Whether `id` is (still) a cloud session record. A converted one is not.
    pub fn is_cloud_session(&self, id: i64) -> bool {
        self.store()
            .ok()
            .and_then(|store| store.remote_session(id).ok().flatten())
            .is_some_and(|s| s.project.cloud().is_some())
    }

    /// Where Claude Code keeps its own sessions (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    pub fn claude_dir(&self) -> &Path {
        &self.claude_dir
    }

    /// Every cloud session bound to `codespace`, archived or not.
    pub fn sessions_in_codespace(&self, codespace: &str) -> Vec<i64> {
        self.sessions_in_cloud(&CloudRef::codespace(codespace))
    }

    /// Every cloud session bound to `cloud`, archived or not.
    pub fn sessions_in_cloud(&self, cloud: &CloudRef) -> Vec<i64> {
        self.store()
            .ok()
            .and_then(|store| {
                // A Host record that backs a Cloud-located local session is not a session of
                // its own: converting it would save a second copy of the same conversation.
                let ids = store.cloud_session_ids(cloud).ok()?;
                Some(
                    ids.into_iter()
                        .filter(|id| {
                            !store
                                .remote_session(*id)
                                .ok()
                                .flatten()
                                .is_some_and(|r| store.host_record_is_hidden(&r))
                        })
                        .collect(),
                )
            })
            .unwrap_or_default()
    }

    /// Converts one cloud session into an archived local session, see the module docs.
    /// BLOCKS (wake, copies); call from a worker thread. On `Err` nothing local was
    /// kept and the cloud record is untouched, so it can be retried.
    pub fn convert_session(
        &self,
        cloud_id: i64,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Converted, String> {
        self.convert_session_confirmed(cloud_id, false, progress)
            .map_err(|e| e.to_string())
    }

    /// [`Self::convert_session`] with a typed error and the files-only confirmation.
    ///
    /// A sandbox whose container was recreated is archived files-only without asking (its
    /// history is gone with the old container). A sandbox whose Host merely could not be
    /// brought back returns [`ConvertError::NeedsFilesOnlyConfirm`] and changes nothing,
    /// unless `files_only_confirmed` is true, which a caller may pass only after the user
    /// agreed to lose access to the conversation. A codespace ignores the flag.
    pub fn convert_session_confirmed(
        &self,
        cloud_id: i64,
        files_only_confirmed: bool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Converted, ConvertError> {
        self.convert_with(cloud_id, files_only_confirmed, Mode::Archive, progress)
    }

    pub(super) fn convert_with(
        &self,
        cloud_id: i64,
        files_only_confirmed: bool,
        mode: Mode,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Converted, ConvertError> {
        let remote = self
            .store()?
            .remote_session(cloud_id)?
            .ok_or("Session not found")?;
        let cloud = remote
            .project
            .cloud()
            .ok_or("This session does not run in a codespace.")?;
        let codespace = cloud.name.clone();
        let local_project = remote
            .project
            .local_project
            .clone()
            .ok_or("This session has no local project to keep a copy in.")?;
        let project_cwd = remote.project.cwd.trim_end_matches('/').to_string();
        valid_remote_path(&project_cwd)?;
        let remote_thread = self
            .store()?
            .thread_dir(cloud_id)?
            .ok_or("This store keeps no session folders, so a local copy cannot be made.")?;
        let machines = self.host.machines()?;
        let machine = machines
            .iter()
            .find(|m| m.environment_id == remote.project.environment_id)
            .cloned();
        let no_machine = || format!("{} machine is no longer connected.", cloud.title());

        // Set when a sandbox can only be archived as files (see the module docs).
        let mut files_only: Option<SandboxTarget> = None;
        match cloud.kind {
            CloudBackendKind::Codespaces => {
                let machine = machine.as_ref().ok_or_else(no_machine)?;
                progress("Waking the codespace");
                self.host
                    .ensure(&machine.id, &AtomicBool::new(false), progress)?;
            }
            // Archiving is the user's own action, so it may resume the sandbox. A connected
            // one is left alone; this also brings the relay and the Host back.
            CloudBackendKind::DockerSandbox => {
                progress("Reconnecting the sandbox");
                match self.reconnect_sandbox_typed(cloud_id, progress, &AtomicBool::new(false)) {
                    Ok(()) => {
                        if machine.is_none() {
                            return Err(no_machine().into());
                        }
                    }
                    // Continuing needs the conversation itself: a files-only copy would
                    // forget the cloud record and lose the turns that are only there.
                    Err(error @ docker_start::ReconnectError::HostUnreachable(_))
                        if mode.continuing() =>
                    {
                        return Err(ConvertError::Failed(format!(
                            "The sandbox cannot be reached ({error}). Start it again and retry."
                        )));
                    }
                    // The conversation that ran in the old container is gone: for a moved
                    // session that is the lost-workspace case (new Claude id, summary).
                    Err(docker_start::ReconnectError::Recreated) if mode.moved().is_some() => {
                        return Err(ConvertError::Failed(SANDBOX_RECREATED.into()));
                    }
                    Err(docker_start::ReconnectError::Recreated) if mode.continuing() => {
                        return Err(ConvertError::Failed(
                            "The sandbox was recreated, so the conversation that ran there is gone. Archive its files only from the archive dialog."
                                .into(),
                        ));
                    }
                    Err(docker_start::ReconnectError::HostUnreachable(reason))
                        if !files_only_confirmed =>
                    {
                        return Err(ConvertError::NeedsFilesOnlyConfirm { reason });
                    }
                    Err(
                        error @ (docker_start::ReconnectError::Recreated
                        | docker_start::ReconnectError::HostUnreachable(_)),
                    ) => {
                        // The paired machine may already name the new Host, so the sandbox is
                        // found by its name.
                        let target = machines
                            .iter()
                            .filter_map(|m| m.sandbox.clone())
                            .find(|s| s.name == cloud.name)
                            .ok_or_else(|| error.to_string())?;
                        crate::remote::validate_sandbox_target(&target)?;
                        progress("The sandbox session cannot be reached; saving its files only");
                        files_only = Some(target);
                    }
                    Err(error) => return Err(error.to_string().into()),
                }
            }
        }
        self.prime_backend(&cloud);

        let mut snapshot = None;
        let mut warnings: Vec<String> = vec![];
        if let Some(host_id) = &remote.session_id {
            let read = |machine: &Machine| -> Result<Value, String> {
                let sync = crate::remote::protocol::read_session_sync(host_id, None, |m, p| {
                    self.host.request(&machine.id, m, p)
                })?;
                let mut value = crate::remote::protocol::apply_session_sync(None, &sync)?;
                crate::remote::attachments::qualify_snapshot(
                    &mut value,
                    &remote.project.environment_id,
                    host_id,
                );
                Ok(value)
            };
            if files_only.is_some() {
                // Best effort and never a wake: a connected Host that still knows the session
                // answers, anything else leaves the copy without history.
                progress("Reading the conversation");
                snapshot = machine.as_ref().and_then(|m| read(m).ok());
            } else {
                progress("Reading the conversation");
                let value = read(machine.as_ref().ok_or_else(no_machine)?)?;
                // Whatever the flags said, a turn the Host is running now is a second writer.
                if mode.continuing() && value["status"] == "running" {
                    return Err(ConvertError::Failed(crate::store::HOST_TURN_RUNNING.into()));
                }
                // Turns the cloud wrote since the last sync are copied first, so the local
                // rows hold everything before the transcript and the code are touched.
                if let Some(local) = mode.moved() {
                    self.store()?.append_remote_rows(local, &value)?;
                }
                snapshot = Some(value);
            }
        }
        if let Some(target) = &files_only {
            // A plain sandbox has no container to recreate: only its Host was unreachable.
            let cause = if target.mode == crate::remote::SandboxMode::Plain {
                "the sandbox's Host could not be reached"
            } else {
                "the sandbox's container was recreated or its Host could not be reached"
            };
            warnings.push(if snapshot.is_some() {
                format!("History may be incomplete: {cause}, so the conversation is whatever the Host still held.")
            } else {
                format!("History may be incomplete: {cause}, so the conversation could not be read and this copy has none. Only the files were saved.")
            });
        }
        // The session may run in its own host worktree: that is the tree to save.
        let workspace_root = self.workspace_root_of(&cloud);
        let session_cwd = snapshot
            .as_ref()
            .and_then(|v| v["session"]["cwd"].as_str())
            .map(str::to_string)
            .or_else(|| {
                // Without a Host the session's own folder is what the store last saw.
                files_only.as_ref()?;
                let settings = self.store().ok()?.agent_settings(cloud_id).ok().flatten()?;
                stored_worktree_cwd(&settings)
            })
            .and_then(|cwd| confine_host_cwd(&cwd, &workspace_root));
        let workspace = session_cwd.clone().unwrap_or_else(|| project_cwd.clone());
        // Where the snapshot script looks: the same tree, at its path in the outer sandbox
        // for a files-only archive.
        let scan_dir = match &files_only {
            Some(target) => outer_path(target, &project_cwd, &workspace)?,
            None => workspace.clone(),
        };
        let mut cwds = vec![project_cwd.clone()];
        cwds.extend(session_cwd);
        let (rows, turns) = match &snapshot {
            Some(value) => {
                let (history, turns) = crate::store::Store::project_remote_history(value)?;
                (history.rows, turns)
            }
            None => (vec![], Default::default()),
        };
        let title = snapshot
            .as_ref()
            .and_then(|v| v["session"]["title"].as_str())
            .filter(|t| !t.trim().is_empty())
            .map_or_else(|| remote.summary.title.clone(), str::to_string);
        let provider_id = snapshot
            .as_ref()
            .and_then(|v| v["session"]["providerSessionId"].as_str())
            .map(str::to_string)
            .or_else(|| remote.summary.provider_id.clone())
            .filter(|id| {
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            });

        let pulled = if let Some(target) = &files_only {
            // `.evidence` is pulled through the Host's machine, which is what is gone.
            let why = if target.mode == crate::remote::SandboxMode::Plain {
                "it is read through the sandbox, which is unreachable"
            } else {
                "it is read through the container, which was recreated or is unreachable"
            };
            EvidencePull {
                session_id: cloud_id,
                kind: cloud.kind,
                instance: codespace.clone(),
                remote_path: String::new(),
                local_dir: remote_thread.join(EVIDENCE_DIR),
                files: vec![],
                message: None,
                warning: Some(format!("The evidence folder was not copied: {why}.")),
            }
        } else {
            progress("Copying evidence");
            self.pull_evidence(cloud_id, None)?
        };
        let evidence_partial = pulled.warning.is_some();
        warnings.extend(pulled.warning);

        let staging = remote_thread.join(format!("convert.{}-{}", std::process::id(), nanos()));
        let result = self.convert_staged(
            Plan {
                remote: &remote,
                cloud: &cloud,
                codespace: &codespace,
                local_project: &local_project,
                workspace: &workspace,
                scan_dir: &scan_dir,
                mode,
                files_only: files_only.is_some(),
                history_saved: files_only.is_none()
                    || snapshot.is_some()
                    || remote.session_id.is_none(),
                cwds: &cwds,
                provider_id: provider_id.as_deref(),
                title: &title,
            },
            rows,
            turns,
            &pulled.local_dir,
            &remote_thread,
            &staging,
            progress,
            &mut warnings,
        );
        let _ = std::fs::remove_dir_all(&staging);
        if result.is_err() {
            release_pins(Path::new(&local_project), &pin_key(cloud_id));
        }
        let mut converted = result?;
        converted.evidence = pulled.files;
        converted.evidence_partial = evidence_partial;
        converted.warnings = warnings;
        Ok(converted)
    }

    #[allow(clippy::too_many_arguments)]
    fn convert_staged(
        &self,
        plan: Plan<'_>,
        rows: Vec<crate::transcript::Row>,
        turns: HashMap<usize, crate::agent::TurnInfo>,
        evidence_dir: &Path,
        remote_thread: &Path,
        staging: &Path,
        progress: &mut dyn FnMut(&str),
        warnings: &mut Vec<String>,
    ) -> Result<Converted, String> {
        let Plan {
            remote,
            cloud,
            codespace,
            local_project,
            workspace,
            scan_dir,
            mode,
            files_only,
            history_saved,
            cwds,
            provider_id,
            title,
        } = plan;
        std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;
        progress("Saving the code state");
        // A files-only archive never looks for Claude's JSONL: it is in the old container.
        let script = SNAPSHOT_SCRIPT.replace("@WS@", scan_dir).replace(
            "@ID@",
            if files_only {
                ""
            } else {
                provider_id.unwrap_or("")
            },
        );
        let backend = if files_only {
            CodeSource::Outer(
                self.sandbox
                    .as_ref()
                    .map(|deps| deps.backend.clone())
                    .ok_or("Docker sandboxes are not available.")?,
                scan_dir,
            )
        } else {
            CodeSource::Cloud(self.backend(cloud.kind)?)
        };
        let noun = cloud.noun();
        let script = if files_only {
            // git may refuse a folder it does not trust; say the workspace is trusted (env
            // config is protected configuration, unlike a repo's own).
            format!(
                "export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0='{scan_dir}'\n{script}"
            )
        } else {
            script
        };
        let out = backend
            .run_script(codespace, &script, true)
            .map_err(|e| outer_git_error(e, files_only))?;
        let snap = parse_snapshot(&out)?;
        let drop_stage = || {
            // The stage lives in /tmp on the codespace; failing to remove it is harmless.
            if snap.stage.starts_with(STAGE_PREFIX) {
                let _ = backend.run_script(codespace, &format!("rm -rf '{}'\n", snap.stage), false);
            }
        };
        if snap.nogit && files_only {
            // The script reports "not a repository" for any git failure; find out why.
            let why = backend
                .run_script(
                    codespace,
                    &format!("git -C '{scan_dir}' rev-parse --git-dir 2>&1 || true\n"),
                    false,
                )
                .unwrap_or_default();
            if why.contains("dubious ownership") {
                drop_stage();
                return Err(format!(
                    "{scan_dir} in the sandbox belongs to another user than the one running git, so its git state cannot be read."
                ));
            }
        }
        if snap.nogit || snap.nohead {
            drop_stage();
            return Err(format!(
                "{scan_dir} on the {noun} is not a git repository with at least one commit, so its files cannot be saved."
            ));
        }
        if snap.size_kb > SIZE_LIMIT_KB {
            drop_stage();
            return Err(format!(
                "The code state is {} MB, over the {} MB limit for a local copy.",
                snap.size_kb / 1024,
                SIZE_LIMIT_KB / 1024
            ));
        }
        let copied = backend.copy_from(codespace, &snap.stage, staging);
        warnings.extend(backend.take_copy_notes());
        drop_stage();
        copied?;
        let staged = staging.join(snap.stage.rsplit('/').next().unwrap_or_default());
        let code = staged.join(CODESPACE_DIR);
        // What the script said it staged must have arrived.
        for (reported, file) in [
            (snap.jsonl, staged.join("claude.jsonl")),
            (snap.bundle, code.join(BUNDLE)),
            (snap.patch, code.join(PATCH)),
            (snap.patches > 0, code.join(PATCHES)),
        ] {
            if reported && !file.exists() {
                return Err(format!(
                    "{} was staged on the {noun} but did not arrive",
                    file.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
        if let Some(note) = check_restorable(
            Path::new(local_project),
            &code,
            &snap,
            &pin_key(remote.summary.id),
        )? {
            warnings.push(note);
        }

        // Everything fetched: from here on a failure rolls the local session back.
        progress("Creating the local copy");
        // A session that already lives here is not created again, and not deleted on failure.
        let created = mode.moved().is_none();
        let previous_provider = match mode.moved() {
            Some(id) => {
                self.store()?
                    .sessions()?
                    .into_iter()
                    .find(|s| s.id == id)
                    .ok_or("The conversation no longer exists")?
                    .provider_id
            }
            None => None,
        };
        let local_id = if let Some(id) = mode.moved() {
            id
        } else {
            let mut store = self.store()?;
            let session = store.create(&remote.summary.provider, local_project)?;
            let local_id = session.id;
            let filled = (|| -> Result<(), String> {
                store.title(local_id, title)?;
                if !rows.is_empty() {
                    store.write_rows(local_id, 0, &rows)?;
                }
                let mut settings = store.agent_settings(remote.summary.id)?.unwrap_or_default();
                settings.worktree_cwd = None;
                settings.worktree_base = None;
                settings.worktree_removed = false;
                settings.new_worktree = false;
                settings.pending_provider_switch = None;
                settings.handoff = None;
                settings.turns = turns;
                store.save_agent_settings(local_id, &settings)?;
                if let Some(item) = &remote.summary.work_item {
                    store.set_work_item(local_id, Some(item))?;
                }
                let mut origin = json!({
                    "codespace": codespace,
                    "remoteCwd": workspace,
                    "hostSessionId": remote.session_id,
                    "environmentId": remote.project.environment_id,
                    "pinKey": pin_key(remote.summary.id),
                    "localProject": local_project,
                    "convertedAt": crate::time::now_ms(),
                });
                if cloud.kind != CloudBackendKind::Codespaces {
                    // `codespace` keeps naming the workspace; this says which backend owns it.
                    origin["backend"] = json!(cloud.kind);
                }
                if files_only {
                    origin["filesOnly"] = json!(true);
                }
                store.save_setting(&origin_key(cloud.kind, local_id), &origin.to_string())
            })();
            if let Err(error) = filled {
                let _ = store.delete(local_id);
                return Err(error);
            }
            local_id
        };
        // Undoes what a failed attempt did to a session that stays (and stays in the cloud).
        let undo_install = |this: &Self| {
            if created {
                return;
            }
            let Ok(store) = this.store() else { return };
            match &previous_provider {
                Some(id) => {
                    let _ = store.provider_id(local_id, id);
                }
                None => {
                    let current = store
                        .sessions()
                        .ok()
                        .and_then(|all| all.into_iter().find(|s| s.id == local_id))
                        .and_then(|s| s.provider_id);
                    let _ = store.reset_provider_id_if(local_id, current.as_deref());
                }
            }
        };
        let local_is_claude = mode.moved().is_none() || {
            self.store()?
                .sessions()?
                .into_iter()
                .find(|s| s.id == local_id)
                .is_some_and(|s| s.provider == HARNESS)
        };
        let tombstone = remote
            .session_id
            .as_deref()
            .map(|host| (remote.project.environment_id.clone(), host.to_string()));
        let mut forked_file: Option<PathBuf> = None;
        let mut finish = || -> Result<Converted, String> {
            let local_thread = self
                .store()?
                .thread_dir(local_id)?
                .ok_or("This store keeps no session folders.")?;
            let dir = local_thread.join(CODESPACE_DIR);
            if mode.continuing() {
                // Every return works in a fresh folder: the files of an earlier return
                // (patches, restored.json) must never be applied or consulted again.
                let _ = std::fs::remove_dir_all(&dir);
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // Claude's raw transcript is always kept, whatever happens to the install below.
            if snap.jsonl {
                std::fs::copy(staged.join("claude.jsonl"), dir.join(RAW_JSONL))
                    .map_err(|e| format!("Cannot keep Claude's transcript: {e}"))?;
            }
            let from: Vec<&str> = cwds.iter().map(String::as_str).collect();
            let mut install = |cwd: &str, warnings: &mut Vec<String>| -> Result<Resume, String> {
                let claude_session = mode.moved().is_none() || local_is_claude;
                let resume = match (provider_id, snap.jsonl) {
                (Some(_), true) if !claude_session => Resume::Missing(
                    "this conversation does not run on Claude, so the cloud transcript is not used"
                        .into(),
                ),
                (Some(id), true) => {
                    match self.install_jsonl(&staged.join("claude.jsonl"), &from, cwd, id)
                    {
                        Ok(resume) => resume,
                        Err(e) => Resume::Missing(e),
                    }
                }
                (Some(_), false) if files_only => Resume::Missing(
                    "Claude's own transcript lived in the old container and was not saved".into(),
                ),
                (Some(_), false) => {
                    Resume::Missing("Claude's own transcript was not found on the codespace".into())
                }
                (None, _) => Resume::Missing("The session never ran Claude".into()),
            };
                if let Resume::Forked(new_id) = &resume {
                    forked_file = self
                        .transcript_dir(cwd)
                        .ok()
                        .map(|(dir, _)| dir.join(format!("{new_id}.jsonl")));
                }
                match &resume {
                Resume::Installed(_) | Resume::AlreadyThere(_) | Resume::Replaced(_) => {
                    if let Some(id) = provider_id {
                        self.store()?.provider_id(local_id, id)?;
                    }
                }
                Resume::Forked(new_id) => {
                    self.store()?.provider_id(local_id, new_id)?;
                    if let Some(id) = provider_id {
                        warnings.push(super::transcript_sync::fork_notice(id, new_id));
                    }
                }
                Resume::Missing(why) if files_only => warnings.push(format!(
                    "Claude cannot resume this conversation locally ({why}); the next message starts a fresh one."
                )),
                Resume::Missing(why) => warnings.push(format!(
                    "Claude cannot resume this conversation locally ({why}); the next message starts a fresh one. The raw transcript is kept in the session folder."
                )),
            }
                if mode.moved().is_some() && matches!(resume, Resume::Missing(_)) {
                    // Never resume the stale local file: it lacks the turns that ran in the cloud.
                    self.start_fresh_with_summary(local_id)?;
                    warnings.push(
                    "The cloud transcript could not be brought back, so the next message starts a new Claude conversation with a summary of this one."
                        .into(),
                );
                }
                Ok(resume)
            };
            let early = if mode.continuing() {
                None
            } else {
                Some(install(local_project, warnings)?)
            };
            let head = snap
                .head
                .as_deref()
                .filter(|h| valid_hex(h))
                .ok_or("The codespace reported no valid HEAD")?;
            for name in [BUNDLE, PATCH] {
                let file = code.join(name);
                if file.is_file() {
                    std::fs::copy(&file, dir.join(name)).map_err(|e| e.to_string())?;
                }
            }
            if code.join(PATCHES).is_dir() {
                copy_tree(&code.join(PATCHES), &dir.join(PATCHES))
                    .map_err(|e| format!("Cannot keep the patch series: {e}"))?;
            }
            let manifest = json!({
                "version": 3,
                "pinKey": pin_key(remote.summary.id),
                "codespace": codespace,
                "remoteCwd": workspace,
                "branch": snap.branch,
                "head": head,
                "base": snap.base.as_deref().filter(|b| valid_hex(b)),
                "baseOnOrigin": snap.base_on_origin,
                "upstream": snap.upstream,
                "remoteUrl": snap.remote,
                "fetchedOrigin": snap.fetched,
                "unpushedCommits": snap.unpushed,
                "stash": snap.stash,
                "refs": snap.refs,
                "bundle": dir.join(BUNDLE).is_file().then_some(BUNDLE),
                "patches": dir.join(PATCHES).is_dir().then_some(PATCHES),
                "patch": dir.join(PATCH).is_file().then_some(PATCH),
                "ignoredNotSaved": snap.ignored.iter().map(|(kb, path)| json!({"path": path, "kib": kb, "regenerable": is_regenerable(path)})).collect::<Vec<_>>(),
                "ignoredTruncated": snap.ignored_truncated,
                "createdAt": crate::time::now_ms(),
            });
            std::fs::write(
                dir.join(MANIFEST),
                serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            warnings.extend(ignored_warning(&snap.ignored, snap.ignored_truncated));
            if !snap.fetched {
                warnings.push(
                    "The codespace could not refresh from origin, so unpushed work may be counted conservatively.".into(),
                );
            }
            if evidence_dir.is_dir() {
                copy_tree(evidence_dir, &local_thread.join(super::EVIDENCE_DIR))
                    .map_err(|e| format!("Cannot keep the evidence: {e}"))?;
            }
            let resume = if mode.continuing() {
                // The code lands (in place or in a new worktree) before the transcript, because
                // Claude finds a session by the folder it runs in; and both land before the
                // cloud record is retired: a failure leaves the session in the cloud.
                let (notes, cwd, in_place) = self.place_code(
                    mode,
                    local_id,
                    local_project,
                    &local_thread,
                    tombstone.as_ref().map_or("", |(_, host)| host.as_str()),
                )?;
                warnings.extend(notes);
                let resume = install(&cwd, warnings)?;
                // Turns that ran in another folder or under another Claude id cannot be
                // rewound from here (their checkpoints describe other directories).
                if let Some(local) = mode.moved() {
                    let started_over = matches!(resume, Resume::Forked(_) | Resume::Missing(_));
                    let store = self.store()?;
                    let count = store.load(local)?.rows.len();
                    let moved_at = store
                        .cloud_location(local)?
                        .cloud()
                        .map_or(count, |l| l.moved_at_row);
                    // Even in place, the turns the cloud wrote ran elsewhere: their file
                    // history belongs to the other machine.
                    if !in_place || started_over || count > moved_at {
                        store.set_returned_gate(local, Some(count))?;
                    }
                }
                resume
            } else {
                self.store()?.set_archived(local_id, true)?;
                early.ok_or("internal: no transcript result")?
            };
            let summary = self
                .store()?
                .sessions()?
                .into_iter()
                .find(|s| s.id == local_id)
                .ok_or("The local copy disappeared")?;
            // Last, so a failure above leaves the cloud record in place. The tombstone keeps
            // the host session from being imported again by a poll or a refresh.
            let store = self.store()?;
            if let Some(local) = mode.moved() {
                // One-way, like Reset to local: the Host copy is tombstoned and the location
                // goes back to Local in the same call.
                store.leave_cloud_inner(local, false)?;
                // Past the point of no return: bookkeeping failures cannot undo the return.
                let _ = store.clear_cloud_move_point(local);
                let _ = store.set_cloud_continue_progress(local, "");
                return Ok(Converted {
                    cloud_id: remote.summary.id,
                    local_id,
                    summary,
                    codespace: codespace.into(),
                    title: title.into(),
                    rows: rows.len(),
                    resume,
                    evidence: vec![],
                    code_dir: Some(dir),
                    ignored: snap.ignored.clone(),
                    ignored_truncated: snap.ignored_truncated,
                    evidence_partial: false,
                    warnings: vec![],
                    transcript_lost: provider_id.is_some() && !snap.jsonl,
                    history_saved,
                });
            }
            if let Some((env, host)) = &tombstone {
                store.set_cloud_converted(env, host, true)?;
            }
            if let Err(e) = store.forget_cloud_session(remote.summary.id) {
                if let Some((env, host)) = &tombstone {
                    let _ = store.set_cloud_converted(env, host, false);
                }
                return Err(e);
            }
            Ok(Converted {
                cloud_id: remote.summary.id,
                local_id,
                summary,
                codespace: codespace.into(),
                title: title.into(),
                rows: rows.len(),
                resume,
                evidence: vec![],
                code_dir: Some(dir),
                ignored: snap.ignored.clone(),
                ignored_truncated: snap.ignored_truncated,
                evidence_partial: false,
                warnings: vec![],
                transcript_lost: provider_id.is_some() && !snap.jsonl,
                history_saved,
            })
        };
        let converted = match finish() {
            Ok(converted) => converted,
            Err(error) => {
                if let Ok(mut store) = self.store() {
                    if let Some((env, host)) = &tombstone {
                        let _ = store.set_cloud_converted(env, host, false);
                    }
                    if created {
                        let _ = store.delete(local_id);
                        // Ids are reused: nothing of this attempt may be inherited.
                        let _ = store.set_cloud_continue_progress(local_id, "");
                    }
                }
                undo_install(self);
                // A retry forks again, so the fork made by this attempt would be an orphan.
                if let Some(file) = &forked_file {
                    let _ = std::fs::remove_file(file);
                }
                return Err(error);
            }
        };
        // The cloud session's own folder only held the evidence, now copied.
        let _ = std::fs::remove_dir_all(evidence_dir);
        let _ = std::fs::remove_dir(remote_thread);
        Ok(converted)
    }

    /// Writes the rewritten JSONL into the local Claude folder (transcript_sync, rule D10):
    /// a fresh id is installed, an extension of the existing file replaces it (the old file
    /// is kept as `<id>.jsonl.pre-cloud`), an equal file is left alone, and a divergent one is
    /// never overwritten: the incoming copy is installed under a new id instead.
    fn install_jsonl(
        &self,
        source: &Path,
        remote_cwds: &[&str],
        local_cwd: &str,
        provider_id: &str,
    ) -> Result<Resume, String> {
        use super::transcript_sync::{decide, fork, Decision};
        let (dir, local) = self.transcript_dir(local_cwd)?;
        let dest = dir.join(format!("{provider_id}.jsonl"));
        let text = std::fs::read_to_string(source)
            .map_err(|e| format!("cannot read Claude's transcript: {e}"))?;
        let text = rewrite_cwd(&text, remote_cwds, &local);
        // A writer that races in after the check is decided again, never trusted blindly.
        for _ in 0..3 {
            let existing = match std::fs::read(&dest) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(format!("cannot read the local transcript: {e}")),
            };
            let decision = decide(existing.as_deref(), text.as_bytes());
            if decision == Decision::AlreadyCurrent {
                return Ok(Resume::AlreadyThere(dest));
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let tmp = dir.join(format!(".{provider_id}.{}.tmp", nanos()));
            match decision {
                Decision::AlreadyCurrent => unreachable!("returned above"),
                Decision::Install => {
                    std::fs::write(&tmp, &text).map_err(|e| e.to_string())?;
                    // A hard link fails when the name exists, so a concurrent writer is never replaced.
                    let linked = std::fs::hard_link(&tmp, &dest);
                    let _ = std::fs::remove_file(&tmp);
                    match linked {
                        Ok(()) => return Ok(Resume::Installed(dest)),
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Decision::Replace => {
                    write_synced(&tmp, &text).map_err(|e| e.to_string())?;
                    let result = replace_keeping_backup(&dir, &tmp, &dest, provider_id);
                    let _ = std::fs::remove_file(&tmp);
                    return result.map(|()| Resume::Replaced(dest));
                }
                Decision::Diverged => {
                    let new_id = uuid::Uuid::new_v4().to_string();
                    let forked = dir.join(format!("{new_id}.jsonl"));
                    std::fs::write(&tmp, fork(&text, &new_id)).map_err(|e| e.to_string())?;
                    let linked = std::fs::hard_link(&tmp, &forked);
                    let _ = std::fs::remove_file(&tmp);
                    linked.map_err(|e| e.to_string())?;
                    return Ok(Resume::Forked(new_id));
                }
            }
        }
        Err("the local transcript keeps changing".into())
    }

    /// The Claude project folder for a local cwd, and the canonical cwd text.
    fn transcript_dir(&self, local_cwd: &str) -> Result<(PathBuf, String), String> {
        let local = std::fs::canonicalize(local_cwd).map_or_else(
            |_| local_cwd.to_string(),
            |p| super::plain_path(p).to_string_lossy().into_owned(),
        );
        let name = claude_project_dir_name(&local);
        if name.len() > CLAUDE_NAME_LIMIT {
            return Err("the local project path is too long for Claude's session folder".into());
        }
        Ok((self.claude_dir.join("projects").join(name), local))
    }
}

/// Writes a file and flushes it to disk, so a rename that follows never exposes a hollow file.
fn write_synced(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// Puts `tmp` in place of `dest` without a moment where `dest` is missing, keeping the
/// previous file as `<id>.jsonl.pre-cloud` (a newer backup replaces an older one: both are
/// prefixes of the new transcript).
fn replace_keeping_backup(
    dir: &Path,
    tmp: &Path,
    dest: &Path,
    provider_id: &str,
) -> Result<(), String> {
    let backup = dir.join(format!("{provider_id}.jsonl.pre-cloud"));
    let staged = dir.join(format!(".{provider_id}.{}.bak", nanos()));
    if std::fs::hard_link(dest, &staged).is_err() {
        std::fs::copy(dest, &staged).map_err(|e| format!("cannot back up the transcript: {e}"))?;
    }
    let renamed = std::fs::rename(&staged, &backup);
    if let Err(e) = renamed {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("cannot back up the transcript: {e}"));
    }
    std::fs::rename(tmp, dest).map_err(|e| e.to_string())
}

/// Where the snapshot script and the copy of its stage run.
enum CodeSource<'a> {
    /// The backend's own workspace (a codespace, or a sandbox's container).
    Cloud(Arc<dyn CloudBackend>),
    /// The outer sandbox around the container, for a files-only archive. The path is the
    /// workspace there, whose owner the scripts run as.
    Outer(Arc<DockerBackend>, &'a str),
}
impl CodeSource<'_> {
    fn run_script(&self, name: &str, script: &str, long: bool) -> Result<String, String> {
        match self {
            Self::Cloud(backend) => backend.run_script(name, script, long),
            Self::Outer(backend, owner) => backend.run_outer_script(name, owner, script, long),
        }
    }
    fn copy_from(&self, name: &str, remote: &str, local: &Path) -> Result<(), String> {
        match self {
            Self::Cloud(backend) => backend.copy_from(name, remote, local),
            Self::Outer(backend, owner) => backend.copy_from_outer(name, owner, remote, local),
        }
    }
    fn take_copy_notes(&self) -> Vec<String> {
        match self {
            Self::Cloud(backend) => backend.take_copy_notes(),
            Self::Outer(backend, _) => CloudBackend::take_copy_notes(&**backend),
        }
    }
}

/// Makes a git failure of the outer sandbox understandable (files-only archives only).
fn outer_git_error(error: String, files_only: bool) -> String {
    if files_only && error.contains("index extension") {
        "The git in the outer sandbox is older than the one in the container, so it cannot read the repository's index. Rebuild or upgrade git in the sandbox, or keep the sandbox and recover the files by hand."
            .to_string()
    } else {
        error
    }
}

/// The outer-sandbox path of `workspace`, a container path at or under `project_cwd`
/// (`/workspaces/<repo>` in the container is `target.workspace` outside).
///
/// Only the workspace folder is a bind mount, so a session whose own folder lies elsewhere in
/// the container (a Host worktree such as `/workspaces/<repo>-worktrees/...`) cannot be read
/// from outside. That is an error, not a silent fallback to the main workspace: archiving the
/// wrong tree and then deleting the sandbox would lose the worktree's work. A custom
/// `workspaceMount` or `workspaceFolder` in the devcontainer config is not supported for a
/// files-only archive in v1: the outer path is always the checkout the start provisioned.
fn outer_path(
    target: &SandboxTarget,
    project_cwd: &str,
    workspace: &str,
) -> Result<String, String> {
    let outer = target.workspace.trim_end_matches('/');
    let path = if workspace == project_cwd {
        outer.to_string()
    } else {
        let rest = workspace
            .strip_prefix(&format!("{project_cwd}/"))
            .ok_or("The session's worktree lived only in the container and cannot be archived; the main workspace can.")?;
        format!("{outer}/{rest}")
    };
    valid_remote_path(&path)?;
    Ok(path)
}

/// What one conversion works on.
struct Plan<'a> {
    remote: &'a RemoteSession,
    cloud: &'a CloudRef,
    /// The workspace name (a codespace or a sandbox).
    codespace: &'a str,
    local_project: &'a str,
    /// The tree that is saved: the session's own cwd on the host.
    workspace: &'a str,
    /// Where `workspace` is read: the same path, or its outer-sandbox path when `files_only`.
    scan_dir: &'a str,
    mode: Mode,
    /// A sandbox archived without its container, see the module docs.
    files_only: bool,
    /// Whether the conversation was read (see [`Converted::history_saved`]).
    history_saved: bool,
    /// Codespace paths to rewrite in Claude's transcript.
    cwds: &'a [String],
    provider_id: Option<&'a str>,
    title: &'a str,
}

/// Runs git with a time limit, no prompts, and git's own message as the error.
pub(super) fn git_run(
    cwd: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    limit: Duration,
) -> Result<String, String> {
    git_run_stdin(cwd, args, envs, limit, None)
}

pub(super) fn git_run_stdin(
    cwd: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    limit: Duration,
    input: Option<&str>,
) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    for (key, value) in envs {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not run Git: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (input, child.stdin.take()) {
        let text = text.to_string();
        std::thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = read(Box::new(child.stdout.take().ok_or("no stdout")?));
    let stderr = read(Box::new(child.stderr.take().ok_or("no stderr")?));
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {} timed out", args.first().unwrap_or(&"")));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        let text = if err.trim().is_empty() { out } else { err };
        Err(text.trim().to_string())
    }
}

pub(super) const GIT_LIMIT: Duration = Duration::from_secs(120);

pub(super) fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    git_run(cwd, args, &[], GIT_LIMIT)
}

/// Before the codespace goes away: the saved commits must be restorable here. Origin is
/// fetched (into this feature's own ref namespace, removed again before returning), and then
/// `head`, every saved ref and, for the patch-series fallback, the base must be present here
/// or come from the bundle. `Ok(Some)` is a warning; `Err` stops the conversion.
pub(super) fn check_restorable(
    project: &Path,
    code: &Path,
    snap: &Snapshot,
    key: &str,
) -> Result<Option<String>, String> {
    let checked = check_restorable_inner(project, code, snap, key);
    if checked.is_err() {
        // Nothing will be restored from a failed capture: release its pins.
        release_pins(project, key);
    }
    checked
}

/// Drops every ref this feature keeps for one converted session (`refs/codespace/<key>/`).
pub(crate) fn release_pins(project: &Path, key: &str) {
    drop_refs(project, &format!("refs/codespace/{key}/"));
}

fn check_restorable_inner(
    project: &Path,
    code: &Path,
    snap: &Snapshot,
    key: &str,
) -> Result<Option<String>, String> {
    let head = snap
        .head
        .as_deref()
        .filter(|h| valid_hex(h))
        .ok_or("The codespace reported no valid HEAD")?;
    if !project.is_dir() {
        return Err(format!(
            "The local project {} is missing, so the saved commits cannot be checked.",
            project.display()
        ));
    }
    let id8 = key;
    fetch_origin(project, id8);
    let _cleanup = OriginRefs { project, id8 };
    let mut pins: Vec<(String, &str)> = vec![("head".into(), head)];
    pins.extend(snap.base.as_deref().map(|b| ("base".to_string(), b)));
    pins.extend(
        snap.refs
            .iter()
            .enumerate()
            .map(|(i, (_, sha))| (format!("ref{i}"), sha.as_str())),
    );
    pin_commits(project, id8, &pins);
    let mut notes = Vec::new();
    if snap.base.is_some() && !snap.base_on_origin {
        notes.push("the base commit is not on origin".to_string());
    }
    let missing = |shas: Vec<&str>| -> Vec<String> {
        shas.into_iter()
            .filter(|sha| !has_commit(project, sha))
            .map(|sha| sha[..12.min(sha.len())].to_string())
            .collect()
    };
    let bundle = code.join(BUNDLE);
    let refs: Vec<&str> = snap.refs.iter().map(|(_, sha)| sha.as_str()).collect();
    if bundle.is_file() {
        match run_git(project, &["bundle", "verify", &bundle.to_string_lossy()]) {
            Ok(_) => {}
            Err(error) => {
                // The bundle does not fit this repository: only the series can still restore
                // the current branch, and it carries neither other branches nor the stash.
                let base = snap.base.as_deref();
                let series_ok = snap.patches > 0
                    && !snap.stash
                    && base.is_some_and(|b| has_commit(project, b))
                    && refs
                        .iter()
                        .all(|sha| *sha == head || has_commit(project, sha));
                if !series_ok {
                    return Err(format!(
                        "The saved commits cannot be restored in this repository: {error}"
                    ));
                }
                notes.push("the saved commits need commits this repository lacks; the patch series will be used".to_string());
            }
        }
    } else {
        // Nothing was bundled: everything saved must already be reachable from origin.
        let mut need = vec![head];
        need.extend(refs);
        let gone = missing(need);
        if !gone.is_empty() {
            return Err(format!(
                "These saved commits are not on origin and were not bundled, so they cannot be restored here: {}",
                gone.join(", ")
            ));
        }
    }
    Ok((!notes.is_empty()).then(|| {
        format!(
            "Restoring the code may need extra care: {}.",
            notes.join(", ")
        )
    }))
}

/// Deletes every ref under `prefix` in one `update-ref --stdin` batch.
fn drop_refs(project: &Path, prefix: &str) {
    let Ok(list) = run_git(project, &["for-each-ref", "--format=%(refname)", prefix]) else {
        return;
    };
    let batch: String = list
        .lines()
        .filter(|l| !l.is_empty())
        .map(|name| format!("delete {name}\n"))
        .collect();
    if !batch.is_empty() {
        let _ = git_run_stdin(
            project,
            &["update-ref", "--stdin"],
            &[],
            GIT_LIMIT,
            Some(&batch),
        );
    }
}

/// Pins the commits restore needs under `refs/codespace/<id8>/keep/`, so they stay
/// reachable here after the bulk copy of origin's branches is dropped.
fn pin_commits(project: &Path, id8: &str, pins: &[(String, &str)]) {
    let batch: String = pins
        .iter()
        .filter(|(_, sha)| has_commit(project, sha))
        .map(|(name, sha)| format!("update refs/codespace/{id8}/keep/{name} {sha}\n"))
        .collect();
    if !batch.is_empty() {
        let _ = git_run_stdin(
            project,
            &["update-ref", "--stdin"],
            &[],
            GIT_LIMIT,
            Some(&batch),
        );
    }
}

/// Removes the `refs/codespace/<id8>/origin/*` copy of origin's branches when dropped.
pub(super) struct OriginRefs<'a> {
    pub(super) project: &'a Path,
    pub(super) id8: &'a str,
}
impl Drop for OriginRefs<'_> {
    fn drop(&mut self) {
        drop_refs(
            self.project,
            &format!("refs/codespace/{}/origin/", self.id8),
        );
    }
}

// ---------------------------------------------------------------------------------------
// Restore

/// What the codespace snapshot recorded (`codespace/manifest.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeManifest {
    pub codespace: String,
    pub branch: Option<String>,
    pub head: String,
    pub base: Option<String>,
    pub remote_url: Option<String>,
    pub bundle: Option<PathBuf>,
    /// The `git format-patch` series `base..head`, a fallback when the bundle cannot be used.
    pub patches: Option<PathBuf>,
    pub patch: Option<PathBuf>,
    /// A restore that created its worktree but could not finish; a retry continues there.
    pub partial: Option<PartialRestore>,
    /// Names this session's refs under `refs/codespace/`.
    pub pin_key: String,
    /// `(ref name, sha)` of the codespace's local branches and stash.
    pub refs: Vec<(String, String)>,
}

/// Where an unfinished restore stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialRestore {
    pub worktree: PathBuf,
    pub branch: String,
    /// `series` (replaying the commits) or `patch` (the uncommitted changes).
    pub step: String,
}

/// The pending code restore of a local session folder, or `None` when there is no manifest or
/// the restore already ran.
pub fn pending_restore(thread_dir: &Path) -> Option<CodeManifest> {
    let dir = thread_dir.join(CODESPACE_DIR);
    let mut partial = None;
    if let Ok(raw) = std::fs::read_to_string(dir.join(RESTORED)) {
        let done: Value = serde_json::from_str(&raw).unwrap_or_default();
        if done["status"] != "partial" {
            return None;
        }
        let worktree = PathBuf::from(done["worktree"].as_str()?);
        partial = Some(PartialRestore {
            worktree,
            branch: done["branch"].as_str()?.to_string(),
            step: done["step"].as_str()?.to_string(),
        });
    }
    let raw = std::fs::read_to_string(dir.join(MANIFEST)).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let head = value["head"].as_str().filter(|h| valid_hex(h))?.to_string();
    let text = |key: &str| {
        value[key]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    let file = |key: &str| {
        let name = value[key].as_str()?;
        // Only the names this module writes; never a path from the file.
        matches!(name, BUNDLE | PATCH | PATCHES)
            .then(|| dir.join(name))
            .filter(|p| p.exists())
    };
    let refs = value["refs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((r[0].as_str()?.to_string(), r[1].as_str()?.to_string())))
        .filter(|(name, sha)| {
            valid_hex(sha) && (name.starts_with("refs/heads/") || name == "refs/stash")
        })
        .collect();
    Some(CodeManifest {
        codespace: text("codespace").unwrap_or_default(),
        branch: text("branch"),
        pin_key: text("pinKey").unwrap_or_else(|| head[..8].to_string()),
        head,
        base: text("base").filter(|b| valid_hex(b)),
        remote_url: text("remoteUrl"),
        bundle: file("bundle"),
        patches: file("patches"),
        patch: file("patch"),
        partial,
        refs,
    })
}

/// Result of [`restore_code`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    pub worktree: PathBuf,
    pub branch: String,
    pub head: String,
    /// The uncommitted changes were applied.
    pub patch_applied: bool,
    /// Git's complaint when commits or the patch did not apply; the worktree is still usable.
    pub conflicts: Option<String>,
}

fn branch_taken(project: &Path, name: &str) -> bool {
    run_git(
        project,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
}

pub(super) fn has_commit(project: &Path, sha: &str) -> bool {
    run_git(project, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok()
}

/// Fetches origin's branches into a namespace this feature owns, so no existing branch or
/// remote-tracking ref moves. Best effort, bounded.
pub(super) fn fetch_origin(project: &Path, id8: &str) {
    let spec = format!("+refs/heads/*:refs/codespace/{id8}/origin/*");
    // Fetching by URL, not by remote name: a named remote also refreshes its tracking refs.
    let Ok(url) = run_git(project, &["remote", "get-url", "origin"]) else {
        return;
    };
    let _ = run_git(
        project,
        &[
            "fetch",
            "-q",
            "--no-tags",
            "--no-write-fetch-head",
            url.trim(),
            &spec,
        ],
    );
}

/// Recreates the codespace's code in a NEW git worktree of `project` (the local project):
/// the branch the codespace was on (or `<branch>-codespace` when that name is taken), the
/// commits from the bundle (or, when the bundle does not fit this repository, from the
/// `format-patch` series applied onto the saved base), then the uncommitted changes from the
/// patch. The main checkout, every existing worktree and every existing branch are left alone:
/// other saved branches and the stash are kept under `refs/codespace/<id>/`, and origin is
/// only ever fetched into that namespace. Runs once per local session: a `restored.json` is
/// written next to the manifest. Problems applying commits or the patch are reported in
/// [`Restored::conflicts`] and do not undo the worktree.
pub fn restore_code(project: &Path, thread_dir: &Path) -> Result<Restored, String> {
    let manifest = pending_restore(thread_dir)
        .ok_or("There is no codespace code to restore for this session.")?;
    if !project.is_dir() {
        return Err(format!("{} is not a folder", project.display()));
    }
    let id8 = manifest.pin_key.clone();
    // Origin's branches are only fetched to find objects; they are removed again at the end.
    let _cleanup = OriginRefs { project, id8: &id8 };
    if let Some(partial) = manifest.partial.clone().filter(|p| p.worktree.is_dir()) {
        // A retry: continue in the worktree the first attempt made.
        let mut restored = Restored {
            worktree: partial.worktree.clone(),
            branch: partial.branch.clone(),
            head: manifest.head.clone(),
            patch_applied: false,
            conflicts: None,
        };
        let replay = (partial.step == "series")
            .then(|| manifest.patches.clone())
            .flatten();
        return finish_restore(project, thread_dir, &manifest, &mut restored, replay, &id8);
    }
    let mut have_head = has_commit(project, &manifest.head);
    let mut unpacked = false;
    if let Some(bundle) = &manifest.bundle {
        let bundle = bundle.to_string_lossy().into_owned();
        let mut verified = run_git(project, &["bundle", "verify", &bundle]);
        if verified.is_err() {
            // Missing prerequisite commits usually just need origin's objects.
            fetch_origin(project, &id8);
            verified = run_git(project, &["bundle", "verify", &bundle]);
        }
        match verified {
            Ok(_) => {
                run_git(project, &["bundle", "unbundle", &bundle])
                    .map_err(|e| format!("Cannot unpack the saved commits: {e}"))?;
                unpacked = true;
                have_head = has_commit(project, &manifest.head);
            }
            Err(e) if manifest.patches.is_none() => {
                return Err(format!("The saved commits cannot be unpacked here: {e}"));
            }
            Err(_) => {}
        }
    }
    if !have_head && !unpacked {
        fetch_origin(project, &id8);
        have_head = has_commit(project, &manifest.head);
    }
    // Where the worktree starts, and whether the commits still have to be replayed.
    let (start, replay) = if have_head {
        (manifest.head.clone(), None)
    } else if let (Some(series), Some(base)) = (&manifest.patches, &manifest.base) {
        if !has_commit(project, base) {
            fetch_origin(project, &id8);
        }
        if !has_commit(project, base) {
            return Err(format!(
                "The base commit {} is not in this repository and could not be fetched.",
                &base[..12.min(base.len())]
            ));
        }
        (base.clone(), Some(series.clone()))
    } else {
        return Err(format!(
            "The commit {} is not in this repository and was not saved. Fetch it and try again.",
            &manifest.head[..12]
        ));
    };
    let wanted = manifest
        .branch
        .clone()
        .unwrap_or_else(|| format!("codespace-{}", &manifest.head[..8]));
    let mut candidates = vec![wanted.clone(), format!("{wanted}-codespace")];
    candidates.extend((2..30).map(|n| format!("{wanted}-codespace-{n}")));
    let mut created = None;
    let mut last_error = String::new();
    for name in candidates {
        if branch_taken(project, &name) {
            continue;
        }
        match crate::git::create_project_worktree(project, &name, Some(&start), false) {
            Ok(tree) => {
                created = Some((name, tree));
                break;
            }
            // A folder or branch checked out elsewhere: try the next name.
            Err(e) if e.contains("already") => last_error = e,
            Err(e) => return Err(e),
        }
    }
    let (branch, tree) = created.ok_or_else(|| {
        if last_error.is_empty() {
            "Every branch name for the restored code is already taken.".to_string()
        } else {
            last_error
        }
    })?;
    let worktree = PathBuf::from(&tree.path);
    let mut restored = Restored {
        worktree: worktree.clone(),
        branch: branch.clone(),
        head: manifest.head.clone(),
        patch_applied: false,
        conflicts: None,
    };
    finish_restore(project, thread_dir, &manifest, &mut restored, replay, &id8)
}

/// Replays the commit series (when `replay`), applies the uncommitted patch and records the
/// outcome in `restored.json`. A step that fails leaves the worktree and records
/// `status: partial` with the step, so a retry continues there.
fn finish_restore(
    project: &Path,
    thread_dir: &Path,
    manifest: &CodeManifest,
    restored: &mut Restored,
    replay: Option<PathBuf>,
    id8: &str,
) -> Result<Restored, String> {
    let worktree = restored.worktree.clone();
    let mut failed_step = None;
    if let Some(series) = replay {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&series)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "patch"))
            .collect();
        files.sort();
        let names: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        // Hooks of the user's repository do not run while commits are replayed.
        let mut args = vec!["-c", "core.hooksPath=/dev/null", "am", "--3way", "--quiet"];
        args.extend(names.iter().map(String::as_str));
        let ident = run_git(&worktree, &["config", "user.email"])
            .map(|e| !e.trim().is_empty())
            .unwrap_or(false);
        let envs: &[(&str, &str)] = if ident {
            &[]
        } else {
            &[
                ("GIT_COMMITTER_NAME", "Monoptah"),
                ("GIT_COMMITTER_EMAIL", "monoptah@localhost"),
            ]
        };
        match git_run(&worktree, &args, envs, Duration::from_secs(300)) {
            Ok(_) => {
                if let Ok(head) = run_git(&worktree, &["rev-parse", "HEAD"]) {
                    restored.head = head.trim().to_string();
                }
            }
            Err(e) => {
                let _ = run_git(&worktree, &["am", "--abort"]);
                failed_step = Some("series");
                restored.conflicts = Some(format!(
                    "The saved commits could not be replayed onto the base in {} (branch {}): {e}. To finish: cd {} && git am --3way {}/*.patch, then apply the uncommitted changes with git apply --binary {}.",
                    worktree.display(),
                    restored.branch,
                    worktree.display(),
                    series.display(),
                    manifest
                        .patch
                        .as_ref()
                        .map_or("(none saved)".to_string(), |p| p.display().to_string()),
                ));
            }
        }
    }
    if let (Some(patch), None) = (&manifest.patch, failed_step) {
        match run_git(
            &worktree,
            &[
                "apply",
                "--binary",
                "--whitespace=nowarn",
                &patch.to_string_lossy(),
            ],
        ) {
            Ok(_) => restored.patch_applied = true,
            Err(e) => {
                failed_step = Some("patch");
                restored.conflicts = Some(format!(
                    "The uncommitted changes did not apply in {} (branch {}): {}. To finish: cd {} && git apply --binary {}",
                    worktree.display(),
                    restored.branch,
                    if e.is_empty() { "no details from git" } else { e.as_str() },
                    worktree.display(),
                    patch.display(),
                ));
            }
        }
    }
    let record = json!({
        "status": if failed_step.is_some() { "partial" } else { "done" },
        "step": failed_step,
        "worktree": restored.worktree,
        "branch": restored.branch,
        "head": restored.head,
        "patchApplied": restored.patch_applied,
        "conflicts": restored.conflicts,
        "restoredAt": crate::time::now_ms(),
    });
    std::fs::write(
        thread_dir.join(CODESPACE_DIR).join(RESTORED),
        record.to_string(),
    )
    .map_err(|e| {
        format!(
            "Restored into {}, but could not record it: {e}",
            worktree.display()
        )
    })?;
    save_refs(project, manifest, id8);
    Ok(restored.clone())
}

/// Every saved branch and the stash get their own ref first, so dropping the pins loses
/// nothing; then the pins are released.
pub(super) fn save_refs(project: &Path, manifest: &CodeManifest, id8: &str) {
    let batch: String = manifest
        .refs
        .iter()
        .filter(|(_, sha)| has_commit(project, sha))
        .map(|(name, sha)| {
            let target = if name == "refs/stash" {
                format!("refs/codespace/{id8}/stash")
            } else {
                format!(
                    "refs/codespace/{id8}/heads/{}",
                    name.trim_start_matches("refs/heads/")
                )
            };
            format!("update {target} {sha}\n")
        })
        .collect();
    if !batch.is_empty() {
        let _ = git_run_stdin(
            project,
            &["update-ref", "--stdin"],
            &[],
            GIT_LIMIT,
            Some(&batch),
        );
    }
    drop_refs(project, &format!("refs/codespace/{id8}/keep/"));
}

/// The pending code restore of local session `id`, if it came from a codespace.
pub fn pending_restore_for(store: &crate::store::Store, id: i64) -> Option<CodeManifest> {
    pending_restore(&store.thread_dir(id).ok()??)
}

/// [`restore_code`] for local session `id`, into a new worktree of the session's project.
pub fn restore_session_code(store: &crate::store::Store, id: i64) -> Result<Restored, String> {
    let project = store
        .sessions()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or("Session not found")?
        .cwd;
    let thread = store
        .thread_dir(id)?
        .ok_or("This store keeps no session folders.")?;
    restore_code(Path::new(&project), &thread)
}
