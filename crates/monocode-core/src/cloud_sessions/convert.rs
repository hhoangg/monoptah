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
use super::*;
use crate::codespaces::valid_remote_path;
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
pub fn origin_key(local_id: i64) -> String {
    format!("codespace-origin:{local_id}")
}

/// Session-independent setting listing what deleting a codespace would lose.
pub fn unsaved_key(codespace: &str) -> String {
    format!("codespace-unsaved:{codespace}")
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

fn valid_hex(text: &str) -> bool {
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
/// escaped for use inside a JSON string. Longer paths are tried first. A match must not be
/// the tail of a longer path (`/tmp/workspaces/app`) or the head of a longer name
/// (`/workspaces/app-two`, `/workspaces/app.git`).
pub(super) fn rewrite_cwd(text: &str, from: &[&str], to: &str) -> String {
    let escaped = serde_json::to_string(to).unwrap_or_default();
    let to = &escaped[1..escaped.len().saturating_sub(1).max(1)];
    let mut froms: Vec<&str> = from.iter().copied().filter(|f| !f.is_empty()).collect();
    froms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    froms.dedup();
    let mut text = text.to_string();
    for from in froms {
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
    /// A file with that id already existed locally and was left alone.
    AlreadyThere(PathBuf),
    /// Not available; the local copy starts a fresh Claude conversation.
    Missing(String),
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

fn nanos() -> u128 {
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
    /// Whether `id` is (still) a cloud session record. A converted one is not.
    pub fn is_cloud_session(&self, id: i64) -> bool {
        self.store()
            .ok()
            .and_then(|store| store.remote_session(id).ok().flatten())
            .is_some_and(|s| s.project.codespace.is_some())
    }

    /// Where Claude Code keeps its own sessions (`CLAUDE_CONFIG_DIR` or `~/.claude`).
    pub fn claude_dir(&self) -> &Path {
        &self.claude_dir
    }

    /// Every cloud session bound to `codespace`, archived or not.
    pub fn sessions_in_codespace(&self, codespace: &str) -> Vec<i64> {
        self.store()
            .ok()
            .and_then(|store| store.cloud_session_ids(codespace).ok())
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
        let remote = self
            .store()?
            .remote_session(cloud_id)?
            .ok_or("Session not found")?;
        let codespace = remote
            .project
            .codespace
            .clone()
            .ok_or("This session does not run in a codespace.")?;
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
        let machine = self
            .host
            .machines()?
            .into_iter()
            .find(|m| m.environment_id == remote.project.environment_id)
            .ok_or("Codespace machine is no longer connected.")?;

        progress("Waking the codespace");
        self.host
            .ensure(&machine.id, &AtomicBool::new(false), progress)?;

        let mut snapshot = None;
        if let Some(host_id) = &remote.session_id {
            progress("Reading the conversation");
            let sync = crate::remote::protocol::read_session_sync(host_id, None, |m, p| {
                self.host.request(&machine.id, m, p)
            })?;
            let mut value = crate::remote::protocol::apply_session_sync(None, &sync)?;
            crate::remote::attachments::qualify_snapshot(
                &mut value,
                &remote.project.environment_id,
                host_id,
            );
            snapshot = Some(value);
        }
        // The session may run in its own host worktree: that is the tree to save.
        let session_cwd = snapshot
            .as_ref()
            .and_then(|v| v["session"]["cwd"].as_str())
            .map(|c| c.trim_end_matches('/').to_string())
            .filter(|c| {
                c.starts_with(&format!("{}/", self.workspace_root)) && valid_remote_path(c).is_ok()
            });
        let workspace = session_cwd.clone().unwrap_or_else(|| project_cwd.clone());
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

        progress("Copying evidence");
        let pulled = self.pull_evidence(cloud_id, None)?;
        let evidence_partial = pulled.warning.is_some();
        let mut warnings: Vec<String> = pulled.warning.into_iter().collect();

        let staging = remote_thread.join(format!("convert.{}-{}", std::process::id(), nanos()));
        let result = self.convert_staged(
            Plan {
                remote: &remote,
                codespace: &codespace,
                local_project: &local_project,
                workspace: &workspace,
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
            codespace,
            local_project,
            workspace,
            cwds,
            provider_id,
            title,
        } = plan;
        std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;
        progress("Saving the code state");
        let script = SNAPSHOT_SCRIPT
            .replace("@WS@", workspace)
            .replace("@ID@", provider_id.unwrap_or(""));
        let out = codespaces::run_script(&*self.gh, codespace, &script, true)?;
        let snap = parse_snapshot(&out)?;
        let drop_stage = |this: &Self| {
            // The stage lives in /tmp on the codespace; failing to remove it is harmless.
            if snap.stage.starts_with(STAGE_PREFIX) {
                let _ = codespaces::run_script(
                    &*this.gh,
                    codespace,
                    &format!("rm -rf '{}'\n", snap.stage),
                    false,
                );
            }
        };
        if snap.nogit || snap.nohead {
            drop_stage(self);
            return Err(format!(
                "{workspace} on the codespace is not a git repository with at least one commit, so its files cannot be saved."
            ));
        }
        if snap.size_kb > SIZE_LIMIT_KB {
            drop_stage(self);
            return Err(format!(
                "The code state is {} MB, over the {} MB limit for a local copy.",
                snap.size_kb / 1024,
                SIZE_LIMIT_KB / 1024
            ));
        }
        let copied = codespaces::copy_from(&*self.gh, codespace, &snap.stage, staging);
        drop_stage(self);
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
                    "{} was staged on the codespace but did not arrive",
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
        let local_id = {
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
                let origin = json!({
                    "codespace": codespace,
                    "remoteCwd": workspace,
                    "hostSessionId": remote.session_id,
                    "environmentId": remote.project.environment_id,
                    "pinKey": pin_key(remote.summary.id),
                    "localProject": local_project,
                    "convertedAt": crate::time::now_ms(),
                });
                store.save_setting(&origin_key(local_id), &origin.to_string())
            })();
            if let Err(error) = filled {
                let _ = store.delete(local_id);
                return Err(error);
            }
            local_id
        };
        let tombstone = remote
            .session_id
            .as_deref()
            .map(|host| (remote.project.environment_id.clone(), host.to_string()));
        let mut finish = || -> Result<Converted, String> {
            let local_thread = self
                .store()?
                .thread_dir(local_id)?
                .ok_or("This store keeps no session folders.")?;
            let dir = local_thread.join(CODESPACE_DIR);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // Claude's raw transcript is always kept, whatever happens to the install below.
            if snap.jsonl {
                std::fs::copy(staged.join("claude.jsonl"), dir.join(RAW_JSONL))
                    .map_err(|e| format!("Cannot keep Claude's transcript: {e}"))?;
            }
            let from: Vec<&str> = cwds.iter().map(String::as_str).collect();
            let resume = match (provider_id, snap.jsonl) {
                (Some(id), true) => {
                    match self.install_jsonl(&staged.join("claude.jsonl"), &from, local_project, id)
                    {
                        Ok(resume) => resume,
                        Err(e) => Resume::Missing(e),
                    }
                }
                (Some(_), false) => {
                    Resume::Missing("Claude's own transcript was not found on the codespace".into())
                }
                (None, _) => Resume::Missing("The session never ran Claude".into()),
            };
            match &resume {
                Resume::Installed(_) | Resume::AlreadyThere(_) => {
                    if let Some(id) = provider_id {
                        self.store()?.provider_id(local_id, id)?;
                    }
                }
                Resume::Missing(why) => warnings.push(format!(
                    "Claude cannot resume this conversation locally ({why}); the next message starts a fresh one. The raw transcript is kept in the session folder."
                )),
            }
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
            self.store()?.set_archived(local_id, true)?;
            let summary = self
                .store()?
                .sessions()?
                .into_iter()
                .find(|s| s.id == local_id)
                .ok_or("The local copy disappeared")?;
            // Last, so a failure above leaves the cloud record in place. The tombstone keeps
            // the host session from being imported again by a poll or a refresh.
            let store = self.store()?;
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
            })
        };
        let converted = match finish() {
            Ok(converted) => converted,
            Err(error) => {
                if let Ok(mut store) = self.store() {
                    if let Some((env, host)) = &tombstone {
                        let _ = store.set_cloud_converted(env, host, false);
                    }
                    let _ = store.delete(local_id);
                }
                return Err(error);
            }
        };
        // The cloud session's own folder only held the evidence, now copied.
        let _ = std::fs::remove_dir_all(evidence_dir);
        let _ = std::fs::remove_dir(remote_thread);
        Ok(converted)
    }

    /// Writes the rewritten JSONL into the local Claude folder, never replacing a file.
    fn install_jsonl(
        &self,
        source: &Path,
        remote_cwds: &[&str],
        local_cwd: &str,
        provider_id: &str,
    ) -> Result<Resume, String> {
        let local = std::fs::canonicalize(local_cwd).map_or_else(
            |_| local_cwd.to_string(),
            |p| p.to_string_lossy().into_owned(),
        );
        let name = claude_project_dir_name(&local);
        if name.len() > CLAUDE_NAME_LIMIT {
            return Err("the local project path is too long for Claude's session folder".into());
        }
        let dir = self.claude_dir.join("projects").join(name);
        let dest = dir.join(format!("{provider_id}.jsonl"));
        if dest.exists() {
            return Ok(Resume::AlreadyThere(dest));
        }
        let text = std::fs::read_to_string(source)
            .map_err(|e| format!("cannot read Claude's transcript: {e}"))?;
        let text = rewrite_cwd(&text, remote_cwds, &local);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!(".{provider_id}.{}.tmp", nanos()));
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        // A hard link fails when the name exists, so a concurrent writer is never replaced.
        let linked = std::fs::hard_link(&tmp, &dest);
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => Ok(Resume::Installed(dest)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(Resume::AlreadyThere(dest))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

/// What one conversion works on.
struct Plan<'a> {
    remote: &'a RemoteSession,
    codespace: &'a str,
    local_project: &'a str,
    /// The tree that is saved: the session's own cwd on the host.
    workspace: &'a str,
    /// Codespace paths to rewrite in Claude's transcript.
    cwds: &'a [String],
    provider_id: Option<&'a str>,
    title: &'a str,
}

/// Runs git with a time limit, no prompts, and git's own message as the error.
fn git_run(
    cwd: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    limit: Duration,
) -> Result<String, String> {
    git_run_stdin(cwd, args, envs, limit, None)
}

fn git_run_stdin(
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

const GIT_LIMIT: Duration = Duration::from_secs(120);

fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
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
struct OriginRefs<'a> {
    project: &'a Path,
    id8: &'a str,
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

fn has_commit(project: &Path, sha: &str) -> bool {
    run_git(project, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok()
}

/// Fetches origin's branches into a namespace this feature owns, so no existing branch or
/// remote-tracking ref moves. Best effort, bounded.
fn fetch_origin(project: &Path, id8: &str) {
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
    // Every saved branch and the stash get their own ref first, so dropping the pins loses
    // nothing; then the worktree's branch holds the restored commits.
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
    Ok(restored.clone())
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
