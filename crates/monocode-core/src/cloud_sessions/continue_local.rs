//! "Continue locally" (design 4.2.3, 4.9 R1/R7, rules D9 and D10): a session that lives in
//! the cloud comes back as ONE local session with ONE Claude id.
//!
//! * A **moved** session (a local session whose location is Cloud) keeps its row and its id.
//!   The rows the cloud wrote were already mirrored into the local database (F4-L); pending
//!   ones are copied first. The location goes back to Local at the very end, in the same
//!   call that tombstones the Host copy (`Store::leave_cloud`).
//! * A session **born in the cloud** gets a new local session that is not archived and that
//!   replaces the cloud row.
//! * Code lands in place when the local tree is exactly the one the move left behind (same
//!   HEAD, same tree id, checked with git), else in a new worktree. Uncommitted local work is
//!   never overwritten: a backup patch is kept and any failure restores the old state.
//! * The transcript goes through `transcript_sync` (extension replaces with a `.pre-cloud`
//!   backup, divergence forks). After a lost workspace nothing is installed: the next turn
//!   starts a new Claude id with a summary, never on the stale file.
//!
//! Every step before the final flip can fail or be cancelled and leaves the session Cloud and
//! consistent; a retry reuses the recorded code placement.
use super::convert::{
    fetch_origin, git_run, has_commit, nanos, pending_restore, restore_code, save_refs, Mode,
    OriginRefs, GIT_LIMIT,
};
use super::*;
use crate::store::{CloudLocation, MovePoint, SessionLocation, WORKSPACE_LOST};
use move_out::head_commit;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where the cloud work went, recorded so a retry does not place it twice.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    /// The Host session of this return (a reused session id never matches).
    host: String,
    /// The folder the session runs in from now on (the transcript is installed for it).
    cwd: String,
    notes: Vec<String>,
    /// The code was applied in the session's own folder (else a new worktree).
    in_place: bool,
}

/// Why the local tree cannot take the cloud work in place.
enum InPlace {
    Applied(String),
    /// A condition does not hold; nothing was touched. The work goes to a new worktree.
    Unsuitable(String),
    /// The tree was touched and could not be put back. Never fall back: report.
    Broken(String),
}

impl CloudSessions {
    /// Brings session `id` back to this computer. For a moved session `id` is the local
    /// session; for a cloud-born one it is the cloud session. BLOCKS (wakes the workspace,
    /// copies, runs git); call from a worker thread, after the user confirmed: syncing from
    /// the Host wakes a stopped workspace.
    pub fn continue_locally(
        &self,
        id: i64,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Converted, ConvertError> {
        let store = self.store()?;
        match store.cloud_location(id)? {
            SessionLocation::Cloud(location) => {
                let backing = store.linked_remote_session(id)?;
                let lost = store.cloud_sync_error(id)?.as_deref() == Some(WORKSPACE_LOST)
                    || backing.is_none();
                if lost {
                    drop(store);
                    return self.continue_without_workspace(id, &location, progress);
                }
                let (record_id, record) = backing.ok_or("The cloud copy is missing")?;
                // The same rule as Reset: a turn the cloud is running is a second writer.
                if record.running_now() {
                    return Err(crate::store::HOST_TURN_RUNNING.into());
                }
                drop(store);
                match self.convert_with(
                    record_id,
                    false,
                    Mode::Continue { moved: Some(id) },
                    progress,
                ) {
                    Err(ConvertError::Failed(why)) if why == super::convert::SANDBOX_RECREATED => {
                        self.continue_without_workspace(id, &location, progress)
                    }
                    other => other,
                }
            }
            SessionLocation::Local => {
                let record = store
                    .remote_session(id)?
                    .ok_or("This conversation is not in the cloud.")?;
                if store.host_record_is_hidden(&record) {
                    return Err("This cloud copy belongs to a conversation on this computer; continue that one.".into());
                }
                drop(store);
                self.convert_with(id, false, Mode::Continue { moved: None }, progress)
            }
        }
    }

    /// The workspace is gone (or its Host record): there is no cloud transcript to bring
    /// back, and the local `<id>.jsonl` lacks the turns written there (design 4.9 R7). So the
    /// session continues on a NEW Claude id: the next turn starts a fresh conversation with a
    /// summary of the rows. The rows already mirrored stay; the code in the folder is left
    /// alone.
    pub fn continue_without_workspace(
        &self,
        id: i64,
        location: &CloudLocation,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Converted, ConvertError> {
        progress("Continuing from the saved messages");
        self.start_fresh_with_summary(id)?;
        let record_id = {
            let store = self.store()?;
            let linked = store.linked_remote_session(id)?.map(|(rid, _)| rid);
            store.leave_cloud_inner(id, false)?;
            let _ = store.clear_cloud_move_point(id);
            let _ = store.set_cloud_continue_progress(id, "");
            linked.unwrap_or(0)
        };
        let store = self.store()?;
        let summary = store
            .sessions()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or("The conversation disappeared")?;
        let warnings = vec![format!(
            "The cloud workspace is gone, so the turns written there after the last sync are lost. The next message starts a new Claude conversation with a summary of this one.{}",
            location
                .pushed_branch
                .as_deref()
                .map(|b| format!(" Code the cloud pushed is on the branch {b} of origin."))
                .unwrap_or_default()
        )];
        Ok(Converted {
            cloud_id: record_id,
            local_id: id,
            rows: store.load(id).map(|h| h.rows.len()).unwrap_or(0),
            title: summary.title.clone(),
            summary,
            codespace: location.cloud.name.clone(),
            resume: Resume::Missing("the cloud workspace is gone".into()),
            evidence: vec![],
            code_dir: None,
            ignored: vec![],
            ignored_truncated: false,
            evidence_partial: false,
            warnings,
            history_saved: true,
        })
    }

    /// See `Store::start_fresh_with_summary`. Idempotent.
    pub(super) fn start_fresh_with_summary(&self, local: i64) -> Result<(), String> {
        self.store()?.start_fresh_with_summary(local)
    }

    /// Puts the cloud's code on this computer. Returns notes for the user, the folder the
    /// session runs in from now on and whether the code went in place. Reuses the recorded
    /// placement of an earlier attempt of the same return.
    pub(super) fn place_code(
        &self,
        mode: Mode,
        local_id: i64,
        local_project: &str,
        local_thread: &Path,
        host: &str,
    ) -> Result<(Vec<String>, String, bool), String> {
        let (point, previous, settings) = {
            let store = self.store()?;
            if let Some(session) = mode.moved() {
                Self::recover_broken(&store, session)?;
            }
            let previous = store
                .cloud_continue_progress(local_id)
                .and_then(|raw| serde_json::from_str::<Progress>(&raw).ok())
                .filter(|p| p.host == host && Path::new(&p.cwd).is_dir());
            let point = mode.moved().and_then(|id| store.cloud_move_point(id));
            (point, previous, store.agent_settings(local_id)?)
        };
        if let Some(done) = previous {
            if !done.in_place {
                // The session must run in the worktree this return made, whatever state its
                // settings are in now.
                let mut settings = settings.unwrap_or_default();
                if settings.worktree_cwd.as_deref() != Some(done.cwd.as_str()) {
                    settings.worktree_cwd = Some(done.cwd.clone());
                    settings.worktree_removed = false;
                    settings.revision += 1;
                    self.store()?.save_agent_settings(local_id, &settings)?;
                }
            }
            return Ok((done.notes, done.cwd, done.in_place));
        }
        let manifest = pending_restore(local_thread)
            .ok_or("There is no cloud code state to bring back for this conversation.")?;
        let project = Path::new(local_project);
        let id8 = manifest.pin_key.clone();
        let mut notes = vec![];
        let mut placed = None;
        if let Some(point) = &point {
            match self.apply_in_place(
                mode.moved().unwrap_or(local_id),
                point,
                &manifest,
                project,
                local_thread,
                &id8,
            ) {
                InPlace::Applied(note) => {
                    placed = Some((point.work_cwd.clone(), note));
                    // Consumed: the menu must not offer to restore it a second time.
                    let _ = std::fs::write(
                        local_thread.join(CODESPACE_DIR).join(RESTORED),
                        json!({"status": "done", "inPlace": true, "restoredAt": crate::time::now_ms()})
                            .to_string(),
                    );
                }
                InPlace::Unsuitable(why) => notes.push(format!(
                    "Your local copy could not take the cloud work in place ({why}), so it went to a new worktree and your files were left alone."
                )),
                InPlace::Broken(why) => return Err(why),
            }
        }
        let in_place = placed.is_some();
        let (cwd, note) = match placed {
            Some(done) => done,
            None => {
                let restored = restore_code(project, local_thread)?;
                let path = restored.worktree.to_string_lossy().into_owned();
                let mut settings = settings.unwrap_or_default();
                settings.worktree_cwd = Some(path.clone());
                settings.worktree_base = None;
                settings.worktree_removed = false;
                settings.new_worktree = false;
                settings.revision += 1;
                self.store()?.save_agent_settings(local_id, &settings)?;
                let mut note = format!(
                    "The code is in a new worktree {path} (branch {}).",
                    restored.branch
                );
                if let Some(conflicts) = restored.conflicts {
                    note.push(' ');
                    note.push_str(&conflicts);
                }
                (path, note)
            }
        };
        notes.push(note);
        let record = Progress {
            host: host.into(),
            cwd: cwd.clone(),
            notes: notes.clone(),
            in_place,
        };
        self.store()?.set_cloud_continue_progress(
            local_id,
            &serde_json::to_string(&record).map_err(|e| e.to_string())?,
        )?;
        Ok((notes, cwd, in_place))
    }

    /// An earlier attempt may have touched the folder. This only LOOKS: it never changes a
    /// file. Nothing changed, or the cloud state fully applied, clears the marker; any other
    /// state is reported with the path of the backup and nothing else happens.
    fn recover_broken(store: &Store, session: i64) -> Result<(), String> {
        let Some(raw) = store.cloud_continue_broken(session) else {
            return Ok(());
        };
        let marker: Marker =
            serde_json::from_str(&raw).map_err(|_| BROKEN_UNREADABLE.to_string())?;
        let root = Path::new(&marker.root);
        let scratch = Path::new(&marker.backup).join("look");
        let _cleanup = RemoveOnDrop(scratch.clone());
        let head = head_commit(root);
        let tree = tree_now(root, &scratch);
        let unchanged = head.as_deref() == Some(marker.head.as_str())
            && tree.as_deref().ok() == Some(marker.tree.as_str());
        let applied = !marker.cloud_head.is_empty()
            && head.as_deref() == Some(marker.cloud_head.as_str())
            && tree.as_deref().ok() == Some(marker.expected.as_str());
        if unchanged || applied {
            // Nothing was lost either way; the backup is no longer needed.
            let _ = std::fs::remove_dir_all(&marker.backup);
            return store.set_cloud_continue_broken(session, "");
        }
        Err(format!(
            "An earlier return from the cloud was interrupted and your local folder {} is now neither as it was nor fully updated (commit {}). Nothing was changed now. A backup of your uncommitted work from before that attempt is in {} (commit {}). Choose Restore backup to put it back (a fresh backup of the current state is taken first), or Discard to forget it.",
            marker.root,
            head.as_deref().unwrap_or("unknown"),
            marker.backup,
            marker.head
        ))
    }

    /// The explicit user action after an interrupted return: put the folder back to the
    /// backed up state. A fresh backup of the CURRENT state is taken first, the branch and
    /// commit must be the ones the marker recorded, and the change goes through the same
    /// refuse-on-conflict path as the return itself. Returns the path of the fresh backup.
    pub fn restore_continue_backup(&self, session: i64) -> Result<PathBuf, String> {
        let store = self.store()?;
        let raw = store
            .cloud_continue_broken(session)
            .ok_or("There is no interrupted return to restore")?;
        let marker: Marker =
            serde_json::from_str(&raw).map_err(|_| BROKEN_UNREADABLE.to_string())?;
        let root = Path::new(&marker.root);
        let fresh = Path::new(&marker.backup).join(format!("current-{}", nanos()));
        move_out::snapshot(root, &fresh)
            .map_err(|e| format!("Could not back up the current state first: {e}"))?;
        let branch = g(root, &["symbolic-ref", "-q", "HEAD"], &[])?;
        if branch.trim() != marker.branch
            || head_commit(root).as_deref() != Some(marker.head.as_str())
        {
            return Err(format!(
                "The branch or commit changed since the interrupted return, so nothing was restored. The current state is saved in {}.",
                fresh.display()
            ));
        }
        let scratch = fresh.join("work");
        std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
        let idx = scratch.join("restore.idx");
        let text = idx.to_string_lossy().into_owned();
        let envs = [("GIT_INDEX_FILE", text.as_str())];
        g(root, &["add", "-A"], &envs)?;
        let current = g(root, &["write-tree"], &envs)?.trim().to_string();
        g(
            root,
            &["read-tree", "-m", "-u", &current, &marker.tree],
            &envs,
        )
        .map_err(|e| {
            format!(
                "Refused, nothing was changed ({e}). The current state is saved in {}.",
                fresh.display()
            )
        })?;
        store.set_cloud_continue_broken(session, "")?;
        Ok(fresh)
    }

    /// The explicit user action: forget the interrupted return and delete its backup. It
    /// cannot be undone. The marker goes first, so a half-deleted backup never blocks.
    pub fn discard_continue_marker(&self, session: i64) -> Result<(), String> {
        let store = self.store()?;
        let backup = store.continue_backup_path(session);
        store.set_cloud_continue_broken(session, "")?;
        if let Some(backup) = backup {
            let _ = std::fs::remove_dir_all(backup);
        }
        Ok(())
    }

    /// The in-place path. Only while the folder is exactly what the move left, with the same
    /// branch checked out and nothing in progress. It never resets or cleans: the working
    /// tree changes through a two-way `read-tree -m -u` that refuses to overwrite anything
    /// the user changed or created, and the branch moves by compare-and-swap.
    fn apply_in_place(
        &self,
        session: i64,
        point: &MovePoint,
        manifest: &super::convert::CodeManifest,
        project: &Path,
        thread: &Path,
        id8: &str,
    ) -> InPlace {
        let unsuitable = |why: &str| InPlace::Unsuitable(why.to_string());
        let work = Path::new(&point.work_cwd);
        if !work.is_dir() {
            return unsuitable("the folder is gone");
        }
        // Every git command runs at the top of the repository (the session may work in a
        // subfolder) and with submodule recursion off.
        let root = match toplevel(work) {
            Ok(root) => root,
            Err(e) => return InPlace::Unsuitable(format!("it is not a git folder: {e}")),
        };
        let branch = match g(&root, &["symbolic-ref", "-q", "HEAD"], &[]) {
            Ok(b) => b.trim().to_string(),
            Err(_) => return unsuitable("HEAD is detached"),
        };
        if point.branch.is_empty() || branch != point.branch {
            return unsuitable("another branch is checked out than at the move");
        }
        for name in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
        ] {
            let path = g(&root, &["rev-parse", "--git-path", name], &[]).unwrap_or_default();
            if root.join(path.trim()).exists() {
                return unsuitable("a merge, rebase, cherry-pick or revert is in progress");
            }
        }
        // Hidden edits (assume-unchanged, skip-worktree) are invisible to the tree check.
        match g(&root, &["ls-files", "-v", "-z"], &[]) {
            Ok(list)
                if list
                    .split('\0')
                    .any(|e| e.chars().next().is_some_and(|c| c != 'H')) =>
            {
                return unsuitable(
                    "some files are hidden from git (assume-unchanged or skip-worktree)",
                );
            }
            Err(e) => return InPlace::Unsuitable(format!("files could not be listed: {e}")),
            Ok(_) => {}
        }
        let scratch = thread.join(format!("check-{}", nanos()));
        let _cleanup = RemoveOnDrop(scratch.clone());
        // Objects first: the slow steps (bundle, fetch) happen before the tree is checked.
        let _origin = OriginRefs {
            project: &root,
            id8,
        };
        let head = manifest.head.as_str();
        if !has_commit(&root, head) {
            if let Some(bundle) = &manifest.bundle {
                let bundle = bundle.to_string_lossy().into_owned();
                if g(&root, &["bundle", "verify", &bundle], &[]).is_err() {
                    fetch_origin(&root, id8);
                }
                if g(&root, &["bundle", "verify", &bundle], &[]).is_ok() {
                    let _ = g(&root, &["bundle", "unbundle", &bundle], &[]);
                }
            }
            if !has_commit(&root, head) {
                fetch_origin(&root, id8);
            }
        }
        if !has_commit(&root, head) {
            return unsuitable("the cloud commits are not available here");
        }
        let expected = match expected_tree(&root, head, manifest.patch.as_deref(), &scratch) {
            Ok(tree) => tree,
            Err(e) => return InPlace::Unsuitable(format!("the cloud state cannot be built: {e}")),
        };
        let current = head_commit(&root);
        // A crash after the apply and before it was recorded: already done.
        if current.as_deref() == Some(head)
            && tree_now(&root, &scratch).is_ok_and(|tree| tree == expected)
        {
            save_refs(project, manifest, id8);
            return InPlace::Applied(format!(
                "The cloud work was already applied to {}.",
                work.display()
            ));
        }
        if current.as_deref() != Some(point.head.as_str()) {
            return unsuitable("the checked out commit changed");
        }
        // Only a fast-forward of what is checked out.
        if g(
            &root,
            &["merge-base", "--is-ancestor", &point.head, head],
            &[],
        )
        .is_err()
        {
            return unsuitable("the branch and the cloud work diverged");
        }
        // The backup of the uncommitted work, kept until the result is verified.
        let backup = thread.join(format!("pre-continue-{}", nanos()));
        let snap = match move_out::snapshot(&root, &backup) {
            Ok(snap) => snap,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&backup);
                return InPlace::Unsuitable(format!("it could not be read: {e}"));
            }
        };
        let drop_backup = || {
            let _ = std::fs::remove_dir_all(&backup);
        };
        if snap.tree != point.tree || snap.head != point.head {
            drop_backup();
            return unsuitable("files changed since the move");
        }
        if !snap.warnings.dirty_submodules.is_empty() {
            drop_backup();
            return unsuitable("a submodule has changes the return cannot carry");
        }
        // Step 1: the temp index holds the folder as it is right now.
        let index = scratch.join("current.idx");
        let index_text = index.to_string_lossy().into_owned();
        let envs = [("GIT_INDEX_FILE", index_text.as_str())];
        let now = g(&root, &["add", "-A"], &envs)
            .and_then(|_| g(&root, &["write-tree"], &envs))
            .map(|t| t.trim().to_string());
        if now.as_deref() != Ok(point.tree.as_str()) {
            drop_backup();
            return unsuitable("files changed while it was checked");
        }
        // Ignored files are never overwritten: refuse when the cloud changes one of them.
        let ignored = g(
            &root,
            &[
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
            ],
            &[],
        )
        .unwrap_or_default();
        let changed = g(
            &root,
            &[
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                &point.tree,
                &expected,
            ],
            &[],
        );
        let changed = match changed {
            Ok(list) => list,
            Err(e) => {
                drop_backup();
                return InPlace::Unsuitable(format!("the change cannot be listed: {e}"));
            }
        };
        let collides = ignored
            .split('\0')
            .filter(|p| !p.is_empty())
            .any(|ignored| {
                changed.split('\0').filter(|p| !p.is_empty()).any(|path| {
                    path == ignored
                        || ignored.starts_with(&format!("{path}/"))
                        || path.starts_with(&format!("{ignored}/"))
                })
            });
        if collides {
            drop_backup();
            return unsuitable("an ignored file of yours is in the way");
        }
        // Write ahead: from here a crash leaves a marker that says where the backup is.
        let marker = Marker {
            root: root.to_string_lossy().into_owned(),
            backup: backup.to_string_lossy().into_owned(),
            head: point.head.clone(),
            tree: point.tree.clone(),
            branch: branch.clone(),
            cloud_head: head.to_string(),
            expected: expected.clone(),
            at: crate::time::now_ms(),
        };
        let marker_text = serde_json::to_string(&marker).unwrap_or_default();
        if self
            .store()
            .and_then(|store| store.set_cloud_continue_broken(session, &marker_text))
            .is_err()
        {
            drop_backup();
            return unsuitable("the safety record could not be written");
        }
        let original_index = g(&root, &["rev-parse", "--git-path", "index"], &[])
            .map(|p| root.join(p.trim()))
            .ok();
        if let Some(index) = &original_index {
            let _ = std::fs::copy(index, backup.join("index.orig"));
        }
        // Steps 2 to 4. Step 2 refuses (exit 128) on a tracked file changed after the
        // index was built and on an untracked file in the way, before touching anything.
        let apply = || -> Result<(), String> {
            g(
                &root,
                &["read-tree", "-m", "-u", &point.tree, &expected],
                &envs,
            )?;
            // Compare-and-swap on the checked out branch.
            g(
                &root,
                &[
                    "update-ref",
                    "-m",
                    "continue locally",
                    &branch,
                    head,
                    &point.head,
                ],
                &[],
            )?;
            // The real index follows HEAD; the working tree is already the expected tree.
            g(&root, &["reset", "-q"], &[])?;
            verify_state(&root, head, &expected, &scratch)
        };
        let result = apply();
        let clear = |this: &Self| {
            if let Ok(store) = this.store() {
                let _ = store.set_cloud_continue_broken(session, "");
            }
        };
        match result {
            Ok(()) => {
                clear(self);
                drop_backup();
                save_refs(project, manifest, id8);
                InPlace::Applied(format!(
                    "The cloud work was applied to {} (nothing there had changed since the move).",
                    work.display()
                ))
            }
            Err(error) => {
                let back = roll_back(
                    &root,
                    &branch,
                    head,
                    &point.head,
                    &point.tree,
                    &backup,
                    &scratch,
                );
                match back {
                    Ok(()) => {
                        clear(self);
                        drop_backup();
                        InPlace::Unsuitable(format!("applying it was refused: {error}"))
                    }
                    Err(e) => InPlace::Broken(format!(
                        "Applying the cloud work to {} failed ({error}) and the old state could not be verified after putting it back ({e}). Your uncommitted work is saved in {} (commit {}). Nothing was deleted.",
                        work.display(),
                        backup.display(),
                        point.head
                    )),
                }
            }
        }
    }
}

/// What an interrupted return left, so a retry can report instead of guessing. Written
/// before the first change to the folder and cleared when the result is verified.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Marker {
    root: String,
    backup: String,
    head: String,
    tree: String,
    branch: String,
    cloud_head: String,
    expected: String,
    at: u64,
}

const BROKEN_UNREADABLE: &str = "An earlier attempt left the local folder in an unknown state and its record cannot be read. Check git status before trying again.";

struct RemoveOnDrop(PathBuf);
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// git at `root` with submodule recursion off.
pub(super) fn g(root: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<String, String> {
    let mut all = vec!["-c", "submodule.recurse=false"];
    all.extend_from_slice(args);
    git_run(root, &all, envs, GIT_LIMIT)
}

fn toplevel(dir: &Path) -> Result<PathBuf, String> {
    g(dir, &["rev-parse", "--show-toplevel"], &[]).map(|out| PathBuf::from(out.trim()))
}

/// Tree id of the working copy at `root`, built the way the move built its own.
fn tree_now(root: &Path, scratch: &Path) -> Result<String, String> {
    let dir = scratch.join(format!("tree-{}", nanos()));
    let result = move_out::snapshot(root, &dir).map(|snap| snap.tree);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn verify_state(root: &Path, head: &str, tree: &str, scratch: &Path) -> Result<(), String> {
    if head_commit(root).as_deref() != Some(head) {
        return Err("the checked out commit is not the expected one".into());
    }
    if tree_now(root, scratch)? != tree {
        return Err("the files are not the expected ones".into());
    }
    Ok(())
}

/// Undoes a failed apply with the same non-destructive moves: the branch goes back by
/// compare-and-swap, the files by a two-way read-tree from what is there now to the move's
/// tree (which refuses rather than overwrite), and the user's own index is put back from its
/// copy. Verified at the end.
fn roll_back(
    root: &Path,
    branch: &str,
    cloud_head: &str,
    old_head: &str,
    old_tree: &str,
    backup: &Path,
    scratch: &Path,
) -> Result<(), String> {
    if head_commit(root).as_deref() == Some(cloud_head) {
        g(
            root,
            &[
                "update-ref",
                "-m",
                "continue locally (undo)",
                branch,
                old_head,
                cloud_head,
            ],
            &[],
        )?;
    }
    let index = scratch.join("undo.idx");
    let text = index.to_string_lossy().into_owned();
    let envs = [("GIT_INDEX_FILE", text.as_str())];
    g(root, &["add", "-A"], &envs)?;
    let now = g(root, &["write-tree"], &envs)?.trim().to_string();
    if now != old_tree {
        g(root, &["read-tree", "-m", "-u", &now, old_tree], &envs)?;
    }
    // The staged state of the user, exactly as it was.
    let saved = backup.join("index.orig");
    if saved.is_file() {
        let real = g(root, &["rev-parse", "--git-path", "index"], &[])?;
        std::fs::copy(&saved, root.join(real.trim())).map_err(|e| e.to_string())?;
        let _ = g(root, &["update-index", "-q", "--refresh"], &[]);
    }
    verify_state(root, old_head, old_tree, scratch)
}

/// The tree the folder must have once the cloud state is applied: the cloud HEAD plus its
/// uncommitted patch, built in a scratch index so no real file is involved.
fn expected_tree(
    root: &Path,
    head: &str,
    patch: Option<&Path>,
    scratch: &Path,
) -> Result<String, String> {
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let index = scratch.join(format!("expected-{}.idx", nanos()));
    let text = index.to_string_lossy().into_owned();
    let envs = [("GIT_INDEX_FILE", text.as_str())];
    let built = (|| {
        g(root, &["read-tree", head], &envs)?;
        if let Some(patch) = patch {
            g(
                root,
                &[
                    "apply",
                    "--cached",
                    "--binary",
                    "--whitespace=nowarn",
                    &patch.to_string_lossy(),
                ],
                &envs,
            )?;
        }
        g(root, &["write-tree"], &envs).map(|out| out.trim().to_string())
    })();
    let _ = std::fs::remove_file(&index);
    built
}
