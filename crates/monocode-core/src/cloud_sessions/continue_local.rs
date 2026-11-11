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
    fetch_origin, git_run, has_commit, nanos, pending_restore, restore_code, run_git, save_refs,
    Mode, OriginRefs, GIT_LIMIT,
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

    /// A earlier attempt touched the folder and could not put it back. Try again to restore
    /// the backed up state; refuse to go on until the folder is verified.
    fn recover_broken(store: &Store, session: i64) -> Result<(), String> {
        let Some(raw) = store.cloud_continue_broken(session) else {
            return Ok(());
        };
        let marker: Broken =
            serde_json::from_str(&raw).map_err(|_| BROKEN_UNREADABLE.to_string())?;
        let root = Path::new(&marker.root);
        let scratch = Path::new(&marker.backup);
        let restored = (|| -> Result<(), String> {
            if head_commit(root).as_deref() == Some(marker.head.as_str())
                && tree_now(root, scratch)? == marker.tree
            {
                return Ok(());
            }
            restore_old(root, &marker.head, marker.patch.as_deref().map(Path::new))?;
            verify_state(root, &marker.head, &marker.tree, scratch)
        })();
        match restored {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(scratch);
                store.set_cloud_continue_broken(session, "")
            }
            Err(e) => Err(format!(
                "An earlier attempt could not put your local files back ({e}). Your uncommitted work is saved as a patch in {} (commit {}). Restore it with git apply --binary, then try again.",
                marker.backup, marker.head
            )),
        }
    }

    /// The in-place path: only while the folder is exactly what the move left, and every
    /// result is verified by tree id. Nothing here runs `clean` on ignored files.
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
        // Every git command runs at the top of the repository: the session may work in a
        // subfolder, and reset, clean and apply must see the same tree the snapshot did.
        let root = match toplevel(work) {
            Ok(root) => root,
            Err(e) => return InPlace::Unsuitable(format!("it is not a git folder: {e}")),
        };
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
                if run_git(&root, &["bundle", "verify", &bundle]).is_err() {
                    fetch_origin(&root, id8);
                }
                if run_git(&root, &["bundle", "verify", &bundle]).is_ok() {
                    let _ = run_git(&root, &["bundle", "unbundle", &bundle]);
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
        if run_git(&root, &["merge-base", "--is-ancestor", &point.head, head]).is_err() {
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
        if snap.tree != point.tree || snap.head != point.head {
            let _ = std::fs::remove_dir_all(&backup);
            return unsuitable("files changed since the move");
        }
        let patch = snap.patch_path.clone();
        // The last look before anything is touched.
        if verify_state(&root, &point.head, &point.tree, &scratch).is_err() {
            let _ = std::fs::remove_dir_all(&backup);
            return unsuitable("files changed while it was checked");
        }
        let apply = || -> Result<(), String> {
            run_git(&root, &["reset", "--hard", "-q", "HEAD"])?;
            // Untracked files that are not ignored: exactly what the move carried.
            run_git(&root, &["clean", "-fdq"])?;
            git_run(&root, &["merge", "--ff-only", "-q", head], &[], GIT_LIMIT)?;
            if let Some(patch) = &manifest.patch {
                run_git(
                    &root,
                    &[
                        "apply",
                        "--binary",
                        "--whitespace=nowarn",
                        &patch.to_string_lossy(),
                    ],
                )?;
            }
            // The exit code of apply is not trusted: the result must be the expected tree.
            verify_state(&root, head, &expected, &scratch)
        };
        match apply() {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&backup);
                save_refs(project, manifest, id8);
                InPlace::Applied(format!(
                    "The cloud work was applied to {} (nothing there had changed since the move).",
                    work.display()
                ))
            }
            Err(error) => {
                let back = restore_old(&root, &point.head, patch.as_deref())
                    .and_then(|()| verify_state(&root, &point.head, &point.tree, &scratch));
                match back {
                    Ok(()) => {
                        let _ = std::fs::remove_dir_all(&backup);
                        InPlace::Unsuitable(format!("applying it failed: {error}"))
                    }
                    Err(e) => {
                        let marker = Broken {
                            root: root.to_string_lossy().into_owned(),
                            backup: backup.to_string_lossy().into_owned(),
                            head: point.head.clone(),
                            tree: point.tree.clone(),
                            patch: patch.map(|p| p.to_string_lossy().into_owned()),
                        };
                        if let (Ok(raw), Ok(store)) = (serde_json::to_string(&marker), self.store())
                        {
                            let _ = store.set_cloud_continue_broken(session, &raw);
                        }
                        InPlace::Broken(format!(
                            "Applying the cloud work to {} failed ({error}) and the old state could not be verified after putting it back ({e}). A patch of your uncommitted work is kept in {} (commit {}). Nothing was deleted; restore it with git apply --binary.",
                            work.display(),
                            backup.display(),
                            point.head
                        ))
                    }
                }
            }
        }
    }
}

/// What a failed restore left, so a retry can finish the job or refuse.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Broken {
    root: String,
    backup: String,
    head: String,
    tree: String,
    patch: Option<String>,
}

const BROKEN_UNREADABLE: &str = "An earlier attempt left the local folder in an unknown state and its record cannot be read. Check git status before trying again.";

struct RemoveOnDrop(PathBuf);
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn toplevel(dir: &Path) -> Result<PathBuf, String> {
    run_git(dir, &["rev-parse", "--show-toplevel"]).map(|out| PathBuf::from(out.trim()))
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

/// Puts the folder back to `head` plus the uncommitted work in `patch`.
fn restore_old(root: &Path, head: &str, patch: Option<&Path>) -> Result<(), String> {
    run_git(root, &["reset", "--hard", "-q", head])?;
    run_git(root, &["clean", "-fdq"])?;
    if let Some(patch) = patch {
        run_git(
            root,
            &[
                "apply",
                "--binary",
                "--whitespace=nowarn",
                &patch.to_string_lossy(),
            ],
        )?;
    }
    Ok(())
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
        git_run(root, &["read-tree", head], &envs, GIT_LIMIT)?;
        if let Some(patch) = patch {
            git_run(
                root,
                &[
                    "apply",
                    "--cached",
                    "--binary",
                    "--whitespace=nowarn",
                    &patch.to_string_lossy(),
                ],
                &envs,
                GIT_LIMIT,
            )?;
        }
        git_run(root, &["write-tree"], &envs, GIT_LIMIT).map(|out| out.trim().to_string())
    })();
    let _ = std::fs::remove_file(&index);
    built
}
