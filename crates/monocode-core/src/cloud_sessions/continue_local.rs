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
    fetch_origin, git_run, has_commit, pending_restore, restore_code, run_git, save_refs, Mode,
    OriginRefs, GIT_LIMIT,
};
use super::*;
use crate::store::{CloudLocation, MovePoint, SessionLocation, WORKSPACE_LOST};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Where the cloud work went, recorded so a retry does not place it twice.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    host: String,
    /// The folder the session runs in from now on (the transcript is installed for it).
    cwd: String,
    notes: Vec<String>,
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
                if record.running_now() && store.cloud_sync_error(id)?.is_none() {
                    return Err(crate::store::HOST_TURN_RUNNING.into());
                }
                drop(store);
                self.convert_with(
                    record_id,
                    false,
                    Mode::Continue { moved: Some(id) },
                    progress,
                )
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
            store.leave_cloud(id)?;
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

    /// The next turn of `local` starts a new Claude conversation with a summary of its rows:
    /// the Claude id is cleared (so nothing resumes a stale file) and the summary is queued
    /// like a provider switch does. Idempotent.
    pub(super) fn start_fresh_with_summary(&self, local: i64) -> Result<(), String> {
        let store = self.store()?;
        let session = store
            .sessions()?
            .into_iter()
            .find(|s| s.id == local)
            .ok_or("The conversation no longer exists")?;
        let rows = store.load(local)?.rows;
        let mut settings = store.agent_settings(local)?.unwrap_or_default();
        settings.handoff = Some(crate::agent::Handoff {
            from: session.provider.clone(),
            brief: crate::response::handoff(&rows),
        });
        settings.revision += 1;
        store.save_agent_settings(local, &settings)?;
        store.reset_provider_id_if(local, session.provider_id.as_deref())
    }

    /// Puts the cloud's code on this computer. Returns notes for the user and the folder the
    /// session runs in from now on. Reuses the recorded placement of an earlier attempt.
    pub(super) fn place_code(
        &self,
        mode: Mode,
        local_id: i64,
        local_project: &str,
        local_thread: &Path,
        host: &str,
    ) -> Result<(Vec<String>, String), String> {
        let (point, previous, settings) = {
            let store = self.store()?;
            let previous = store
                .cloud_continue_progress(local_id)
                .and_then(|raw| serde_json::from_str::<Progress>(&raw).ok())
                .filter(|p| p.host == host && Path::new(&p.cwd).is_dir());
            let point = mode.moved().and_then(|id| store.cloud_move_point(id));
            (point, previous, store.agent_settings(local_id)?)
        };
        if let Some(done) = previous {
            return Ok((done.notes, done.cwd));
        }
        let manifest = pending_restore(local_thread)
            .ok_or("There is no cloud code state to bring back for this conversation.")?;
        let project = Path::new(local_project);
        let id8 = manifest.pin_key.clone();
        let mut notes = vec![];
        let mut placed = None;
        if let Some(point) = &point {
            match self.apply_in_place(point, &manifest, project, local_thread, &id8) {
                InPlace::Applied(note) => placed = Some((point.work_cwd.clone(), note)),
                InPlace::Unsuitable(why) => notes.push(format!(
                    "Your local copy changed since the move ({why}), so the cloud work went to a new worktree and your files were left alone."
                )),
                InPlace::Broken(why) => return Err(why),
            }
        }
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
        };
        self.store()?.set_cloud_continue_progress(
            local_id,
            &serde_json::to_string(&record).map_err(|e| e.to_string())?,
        )?;
        Ok((notes, cwd))
    }

    /// The in-place path: only while the folder is exactly what the move left.
    fn apply_in_place(
        &self,
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
        if move_out::head_commit(work).as_deref() != Some(point.head.as_str()) {
            return unsuitable("the checked out commit changed");
        }
        let backup = thread.join("pre-continue");
        let _ = std::fs::remove_dir_all(&backup);
        let snap = match move_out::snapshot(work, &backup) {
            Ok(snap) => snap,
            Err(e) => return InPlace::Unsuitable(format!("it could not be read: {e}")),
        };
        if snap.tree != point.tree || snap.head != point.head {
            let _ = std::fs::remove_dir_all(&backup);
            return unsuitable("files changed");
        }
        // The objects of the cloud's HEAD, from its bundle or from origin.
        let _origin = OriginRefs { project: work, id8 };
        let head = manifest.head.as_str();
        if !has_commit(work, head) {
            if let Some(bundle) = &manifest.bundle {
                let bundle = bundle.to_string_lossy().into_owned();
                if run_git(work, &["bundle", "verify", &bundle]).is_err() {
                    fetch_origin(work, id8);
                }
                if run_git(work, &["bundle", "verify", &bundle]).is_ok() {
                    let _ = run_git(work, &["bundle", "unbundle", &bundle]);
                }
            }
            if !has_commit(work, head) {
                fetch_origin(work, id8);
            }
        }
        if !has_commit(work, head) {
            return unsuitable("the cloud commits are not available here");
        }
        // Only a fast-forward of what is checked out.
        if run_git(work, &["merge-base", "--is-ancestor", &point.head, head]).is_err() {
            return unsuitable("the branch and the cloud work diverged");
        }
        let patch = snap.patch_path.clone();
        let restore_old = |work: &Path| -> Result<(), String> {
            run_git(work, &["reset", "--hard", "-q", &point.head])?;
            run_git(work, &["clean", "-fdq"])?;
            if let Some(patch) = &patch {
                run_git(
                    work,
                    &[
                        "apply",
                        "--binary",
                        "--whitespace=nowarn",
                        &patch.to_string_lossy(),
                    ],
                )?;
            }
            Ok(())
        };
        // The tree equals the move's tree, and the cloud state descends from it, so the cloud
        // state replaces it. The backup patch above rebuilds the old state if anything fails.
        let apply = || -> Result<(), String> {
            run_git(work, &["reset", "--hard", "-q", "HEAD"])?;
            run_git(work, &["clean", "-fdq"])?;
            git_run(work, &["merge", "--ff-only", "-q", head], &[], GIT_LIMIT)?;
            if let Some(patch) = &manifest.patch {
                run_git(
                    work,
                    &[
                        "apply",
                        "--binary",
                        "--whitespace=nowarn",
                        &patch.to_string_lossy(),
                    ],
                )?;
            }
            Ok(())
        };
        match apply() {
            Ok(()) => {
                save_refs(project, manifest, id8);
                InPlace::Applied(format!(
                    "The cloud work was applied to {} (nothing there had changed since the move).",
                    work.display()
                ))
            }
            Err(error) => match restore_old(work) {
                Ok(()) => InPlace::Unsuitable(format!("applying it failed: {error}")),
                Err(e) => InPlace::Broken(format!(
                    "Applying the cloud work to {} failed ({error}) and the old state could not be put back ({e}). A patch of your uncommitted work is in {}; the commit is {}.",
                    work.display(),
                    backup.display(),
                    point.head
                )),
            },
        }
    }
}
