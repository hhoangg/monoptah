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

/// Folder inside the local session folder that holds the code state.
pub const CODESPACE_DIR: &str = "codespace";
pub const MANIFEST: &str = "manifest.json";
pub const BUNDLE: &str = "commits.bundle";
pub const PATCH: &str = "changes.patch";
/// Written by [`restore_code`]; its presence means the action was used.
pub const RESTORED: &str = "restored.json";
/// Session setting recording where a converted session came from.
pub fn origin_key(local_id: i64) -> String {
    format!("codespace-origin:{local_id}")
}

/// Runs on the codespace with `sh -s`. `@WS@` and `@ID@` are substituted (validated first).
/// Prints `key=value` lines; everything it stages lives under the printed `stage`.
const SNAPSHOT_SCRIPT: &str = r#"set -eu
ws='@WS@'
id='@ID@'
stage=$(mktemp -d /tmp/monoptah-archive.XXXXXX)
echo "stage=$stage"
if [ -n "$id" ]; then
  f=$(find "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects" -maxdepth 2 -type f -name "$id.jsonl" 2>/dev/null | head -n 1 || true)
  if [ -n "$f" ]; then cp "$f" "$stage/claude.jsonl"; echo "jsonl=1"; fi
fi
if [ -d "$ws" ] && git -C "$ws" rev-parse --git-dir >/dev/null 2>&1; then
  cd "$ws"
  head=$(git rev-parse -q --verify HEAD 2>/dev/null || true)
  if [ -n "$head" ]; then
    mkdir -p "$stage/codespace"
    echo "head=$head"
    echo "branch=$(git symbolic-ref --short -q HEAD 2>/dev/null || true)"
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
    if [ -n "$up" ]; then base=$(git merge-base HEAD "$up" 2>/dev/null || true); fi
    echo "base=$base"
    n=$(git rev-list --count HEAD --not --remotes 2>/dev/null || echo 0)
    echo "unpushed=$n"
    if [ "$n" -gt 0 ]; then
      git bundle create "$stage/codespace/commits.bundle" HEAD --not --remotes >/dev/null 2>&1
      echo "bundle=1"
    fi
    idx="$stage/index.tmp"
    GIT_INDEX_FILE="$idx" git read-tree HEAD
    GIT_INDEX_FILE="$idx" git add -A >/dev/null 2>&1
    GIT_INDEX_FILE="$idx" git diff --cached --binary HEAD > "$stage/codespace/changes.patch"
    rm -f "$idx"
    if [ -s "$stage/codespace/changes.patch" ]; then echo "patch=1"; else rm -f "$stage/codespace/changes.patch"; fi
  else
    echo "nohead=1"
  fi
fi
"#;

/// Facts the snapshot script reported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) stage: String,
    pub(super) jsonl: bool,
    pub(super) head: Option<String>,
    pub(super) branch: Option<String>,
    pub(super) remote: Option<String>,
    pub(super) upstream: Option<String>,
    pub(super) base: Option<String>,
    pub(super) unpushed: u64,
    pub(super) bundle: bool,
    pub(super) patch: bool,
}

pub(super) fn parse_snapshot(out: &str) -> Result<Snapshot, String> {
    let mut snap = Snapshot::default();
    for line in out.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let text = || (!value.is_empty()).then(|| value.to_string());
        match key {
            "stage" => snap.stage = value.into(),
            "jsonl" => snap.jsonl = value == "1",
            "head" => snap.head = text(),
            "branch" => snap.branch = text(),
            "remote" => snap.remote = text(),
            "upstream" => snap.upstream = text(),
            "base" => snap.base = text(),
            "unpushed" => snap.unpushed = value.parse().unwrap_or(0),
            "bundle" => snap.bundle = value == "1",
            "patch" => snap.patch = value == "1",
            _ => {}
        }
    }
    if snap.stage.is_empty() {
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

/// Replaces `from` (a path, matched on a boundary) by `to` in a JSONL text. `to` is escaped
/// for use inside a JSON string.
pub(super) fn rewrite_cwd(text: &str, from: &str, to: &str) -> String {
    let escaped = serde_json::to_string(to).unwrap_or_default();
    let to = &escaped[1..escaped.len().saturating_sub(1).max(1)];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(from) {
        let after = &rest[at + from.len()..];
        let boundary = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        out.push_str(&rest[..at]);
        out.push_str(if boundary { to } else { from });
        rest = after;
    }
    out.push_str(rest);
    out
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
    /// The `codespace/` folder when the workspace is a git repository with commits.
    pub code_dir: Option<PathBuf>,
    /// Problems that did not stop the conversion.
    pub warnings: Vec<String>,
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
        let workspace = remote.project.cwd.trim_end_matches('/').to_string();
        valid_remote_path(&workspace)?;
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
        let mut warnings: Vec<String> = pulled.warning.into_iter().collect();

        let staging = remote_thread.join(format!("convert.{}-{}", std::process::id(), nanos()));
        let result = self.convert_staged(
            &remote,
            &codespace,
            &local_project,
            &workspace,
            provider_id.as_deref(),
            &title,
            rows,
            turns,
            &pulled.local_dir,
            &remote_thread,
            &staging,
            progress,
            &mut warnings,
        );
        let _ = std::fs::remove_dir_all(&staging);
        let mut converted = result?;
        converted.evidence = pulled.files;
        converted.warnings = warnings;
        Ok(converted)
    }

    #[allow(clippy::too_many_arguments)]
    fn convert_staged(
        &self,
        remote: &RemoteSession,
        codespace: &str,
        local_project: &str,
        workspace: &str,
        provider_id: Option<&str>,
        title: &str,
        rows: Vec<crate::transcript::Row>,
        turns: HashMap<usize, crate::agent::TurnInfo>,
        evidence_dir: &Path,
        remote_thread: &Path,
        staging: &Path,
        progress: &mut dyn FnMut(&str),
        warnings: &mut Vec<String>,
    ) -> Result<Converted, String> {
        std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;
        progress("Saving the code state");
        let script = SNAPSHOT_SCRIPT
            .replace("@WS@", workspace)
            .replace("@ID@", provider_id.unwrap_or(""));
        let out = codespaces::run_script(&*self.gh, codespace, &script, true)?;
        let snap = parse_snapshot(&out)?;
        let copied = codespaces::copy_from(&*self.gh, codespace, &snap.stage, staging);
        // The stage lives in /tmp on the codespace; failing to remove it is harmless.
        let _ = codespaces::run_script(
            &*self.gh,
            codespace,
            &format!("rm -rf '{}'\n", snap.stage),
            false,
        );
        copied?;
        let stage_name = snap.stage.rsplit('/').next().unwrap_or_default();
        let staged = staging.join(stage_name);

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
        let mut finish = || -> Result<Converted, String> {
            let local_thread = self
                .store()?
                .thread_dir(local_id)?
                .ok_or("This store keeps no session folders.")?;
            std::fs::create_dir_all(&local_thread).map_err(|e| e.to_string())?;
            let resume = match (provider_id, snap.jsonl) {
                (Some(id), true) => {
                    match self.install_jsonl(
                        &staged.join("claude.jsonl"),
                        workspace,
                        local_project,
                        id,
                    ) {
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
                    "Claude cannot resume this conversation locally ({why}); the next message starts a fresh one."
                )),
            }
            let mut code_dir = None;
            if let Some(head) = snap.head.as_deref().filter(|h| valid_hex(h)) {
                let dir = local_thread.join(CODESPACE_DIR);
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let from = staged.join(CODESPACE_DIR);
                for name in [BUNDLE, PATCH] {
                    let file = from.join(name);
                    if file.is_file() {
                        std::fs::copy(&file, dir.join(name)).map_err(|e| e.to_string())?;
                    }
                }
                let manifest = json!({
                    "version": 1,
                    "codespace": codespace,
                    "remoteCwd": workspace,
                    "branch": snap.branch,
                    "head": head,
                    "base": snap.base.as_deref().filter(|b| valid_hex(b)),
                    "upstream": snap.upstream,
                    "remoteUrl": snap.remote,
                    "unpushedCommits": snap.unpushed,
                    "bundle": dir.join(BUNDLE).is_file().then_some(BUNDLE),
                    "patch": dir.join(PATCH).is_file().then_some(PATCH),
                    "createdAt": crate::time::now_ms(),
                });
                std::fs::write(
                    dir.join(MANIFEST),
                    serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                code_dir = Some(dir);
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
            Ok(Converted {
                cloud_id: remote.summary.id,
                local_id,
                summary,
                codespace: codespace.into(),
                title: title.into(),
                rows: rows.len(),
                resume,
                evidence: vec![],
                code_dir,
                warnings: vec![],
            })
        };
        let converted = match finish() {
            Ok(converted) => converted,
            Err(error) => {
                let _ = self.store().map(|mut store| store.delete(local_id));
                return Err(error);
            }
        };
        let store = self.store()?;
        store.forget_cloud_session(remote.summary.id)?;
        drop(store);
        // The cloud session's own folder only held the evidence, now copied.
        let _ = std::fs::remove_dir_all(evidence_dir);
        let _ = std::fs::remove_dir(remote_thread);
        Ok(converted)
    }

    /// Writes the rewritten JSONL into the local Claude folder, never replacing a file.
    fn install_jsonl(
        &self,
        source: &Path,
        remote_cwd: &str,
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
        let text = rewrite_cwd(&text, remote_cwd, &local);
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
    pub patch: Option<PathBuf>,
}

/// The pending code restore of a local session folder, or `None` when there is no manifest or
/// the restore already ran.
pub fn pending_restore(thread_dir: &Path) -> Option<CodeManifest> {
    let dir = thread_dir.join(CODESPACE_DIR);
    if dir.join(RESTORED).exists() {
        return None;
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
        // Only the two names this module writes; never a path from the file.
        matches!(name, BUNDLE | PATCH)
            .then(|| dir.join(name))
            .filter(|p| p.is_file())
    };
    Some(CodeManifest {
        codespace: text("codespace").unwrap_or_default(),
        branch: text("branch"),
        head,
        base: text("base").filter(|b| valid_hex(b)),
        remote_url: text("remoteUrl"),
        bundle: file("bundle"),
        patch: file("patch"),
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
    /// Git's complaint when the patch did not apply; the worktree is still at `head`.
    pub conflicts: Option<String>,
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    crate::git::git(cwd, args)
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

/// Recreates the codespace's code in a NEW git worktree of `project` (the local project):
/// the branch the codespace was on (or `<branch>-codespace` when that name is taken), the
/// unpushed commits from the bundle, then the uncommitted changes from the patch. The main
/// checkout and every existing worktree are left alone. Runs once per local session: a
/// `restored.json` is written next to the manifest. Patch problems are reported in
/// [`Restored::conflicts`] and do not undo the worktree.
pub fn restore_code(project: &Path, thread_dir: &Path) -> Result<Restored, String> {
    let manifest = pending_restore(thread_dir)
        .ok_or("There is no codespace code to restore for this session.")?;
    if !project.is_dir() {
        return Err(format!("{} is not a folder", project.display()));
    }
    if let Some(bundle) = &manifest.bundle {
        let bundle = bundle.to_string_lossy().into_owned();
        let mut verified = run_git(project, &["bundle", "verify", &bundle]);
        if verified.is_err() {
            // Missing prerequisite commits usually just need a fetch.
            let _ = run_git(project, &["fetch", "--quiet", "origin"]);
            verified = run_git(project, &["bundle", "verify", &bundle]);
        }
        verified.map_err(|e| format!("The saved commits cannot be unpacked here: {e}"))?;
        run_git(project, &["bundle", "unbundle", &bundle])
            .map_err(|e| format!("Cannot unpack the saved commits: {e}"))?;
    }
    if !has_commit(project, &manifest.head) {
        let _ = run_git(project, &["fetch", "--quiet", "origin"]);
        if !has_commit(project, &manifest.head) {
            return Err(format!(
                "The commit {} is not in this repository and was not saved. Fetch it and try again.",
                &manifest.head[..12.min(manifest.head.len())]
            ));
        }
    }
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
        match crate::git::create_project_worktree(project, &name, Some(&manifest.head), false) {
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
    if let Some(patch) = &manifest.patch {
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
                restored.conflicts = Some(if e.is_empty() {
                    "The uncommitted changes could not be applied.".into()
                } else {
                    e
                })
            }
        }
    }
    let record = json!({
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
    .map_err(|e| format!("Restored, but could not record it: {e}"))?;
    Ok(restored)
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
