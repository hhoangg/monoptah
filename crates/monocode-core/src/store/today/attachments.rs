//! Notes with files: each file is copied into the app data directory before
//! the note is written (see `today::attachments`).
use super::{actor_sees, load_task, sql, Store, NOTE_MAX, NOT_FOUND};
use crate::attachment::write_attachment_limited;
use crate::today::attachments::{
    check_file, describe, display_name, note_dir, resolve, MAX_FILE_BYTES, MAX_PER_NOTE,
};
use crate::today::{Actor, Note, NoteAttachment};
use rusqlite::params;
use std::path::{Path, PathBuf};

/// A written note and the files that could not be attached to it, one message
/// each that names the file and says why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteSaved {
    pub note: Note,
    pub skipped: Vec<String>,
}

/// A validated note that is not written yet: the task and note ids are chosen
/// and the copy destination is known. Copying files needs no store, so a caller
/// can release the store lock while the (possibly large) files are copied.
#[derive(Clone, Debug)]
pub struct NotePlan {
    task_id: String,
    note_id: String,
    relative: PathBuf,
    root: Option<PathBuf>,
    body: String,
}

impl NotePlan {
    /// Copies `files` under the plan's note folder. A file that cannot be copied
    /// is left out and named in the second list. Touches no store.
    pub fn copy(&self, files: &[PathBuf]) -> Result<(Vec<NoteAttachment>, Vec<String>), String> {
        if files.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let root = self
            .root
            .as_deref()
            .ok_or("Attachment storage is unavailable")?;
        Ok(copy_files(root, &self.relative, files))
    }
    /// Removes the plan's note folder and any copies in it. Call it when the
    /// plan will not be committed.
    pub fn discard(&self) {
        if let Some(root) = &self.root {
            let _ = std::fs::remove_dir_all(root.join(&self.relative));
        }
    }
}

impl Store {
    /// Appends a note with copies of `files`. The body may be empty when at
    /// least one file is attached. A file that cannot be copied is left out and
    /// named in `skipped`; the note fails only when nothing at all would be
    /// written. More than `MAX_PER_NOTE` files is refused outright. This is the
    /// three steps below in one call; a caller that must not hold the store
    /// while copying runs them itself.
    pub fn today_note_with_files(
        &self,
        id: &str,
        body: &str,
        files: &[PathBuf],
        actor: &Actor,
        now: u64,
    ) -> Result<NoteSaved, String> {
        let plan = self.today_note_plan(id, body, files.len(), actor, now)?;
        let (attachments, skipped) = plan.copy(files)?;
        self.today_note_commit(plan, attachments, skipped, actor, now)
    }

    /// Step 1: checks the body, the number of files and the task, and picks the
    /// note's id and folder. Writes nothing.
    pub fn today_note_plan(
        &self,
        id: &str,
        body: &str,
        file_count: usize,
        actor: &Actor,
        now: u64,
    ) -> Result<NotePlan, String> {
        self.today_writable()?;
        let body = body.trim().to_string();
        if body.is_empty() && file_count == 0 {
            return Err("note cannot be empty".into());
        }
        if body.chars().count() > NOTE_MAX {
            return Err(format!("A note is at most {NOTE_MAX} characters"));
        }
        if file_count > MAX_PER_NOTE {
            return Err(format!("A note holds at most {MAX_PER_NOTE} attachments"));
        }
        self.today_roll(now)?;
        self.today_ready()?;
        let task = load_task(&self.conn, id)?
            .filter(|task| actor_sees(&self.conn, actor, task))
            .ok_or_else(|| NOT_FOUND.to_string())?;
        let note_id = format!("note-{}", uuid::Uuid::new_v4());
        Ok(NotePlan {
            relative: note_dir(&task.id, &note_id),
            task_id: task.id,
            note_id,
            root: self.data_dir().map(Path::to_path_buf),
            body,
        })
    }

    /// Step 3: writes the note with the files step 2 copied. On any failure
    /// the copies are removed.
    pub fn today_note_commit(
        &self,
        plan: NotePlan,
        attachments: Vec<NoteAttachment>,
        skipped: Vec<String>,
        actor: &Actor,
        now: u64,
    ) -> Result<NoteSaved, String> {
        if plan.body.is_empty() && attachments.is_empty() {
            plan.discard();
            return Err(skipped.join("\n"));
        }
        let mut stored = attachments.clone();
        for attachment in &mut stored {
            attachment.path = None;
        }
        let json = (!stored.is_empty())
            .then(|| serde_json::to_string(&stored))
            .transpose();
        let written = json.map_err(|e| e.to_string()).and_then(|json| {
            let day = self.today_roll(now)?;
            self.today_tx(|conn| {
                let task = load_task(conn, &plan.task_id)?
                    .filter(|task| actor_sees(conn, actor, task))
                    .ok_or_else(|| NOT_FOUND.to_string())?;
                let note = Note {
                    id: plan.note_id.clone(),
                    task_id: task.id,
                    day,
                    at: now,
                    mono_id: match actor {
                        Actor::User | Actor::Session(_) => None,
                        Actor::Mono(caller) => Some(caller.id.clone()),
                    },
                    body: plan.body.clone(),
                    attachments,
                    edited_at: None,
                    session_id: match actor {
                        Actor::Session(id) => Some(*id),
                        _ => None,
                    },
                };
                conn.execute(
                    "INSERT INTO today_notes (id, task_id, day, at, mono_id, body, attachments_json, session_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        note.id,
                        note.task_id,
                        note.day,
                        note.at.min(i64::MAX as u64) as i64,
                        note.mono_id,
                        note.body,
                        json,
                        note.session_id
                    ],
                )
                .map_err(sql)?;
                Ok(note)
            })
        });
        match written {
            Ok(note) => Ok(NoteSaved { note, skipped }),
            Err(error) => {
                // Nothing points at the copies: take them back.
                plan.discard();
                Err(error)
            }
        }
    }

    /// The absolute path of a stored copy, or `None` when the store has no data
    /// directory or `file` is not a stored shape. Does not check it exists.
    pub fn today_attachment_path(&self, attachment: &NoteAttachment) -> Option<PathBuf> {
        resolve(self.data_dir()?, &attachment.file)
    }

    pub(super) fn with_attachment_paths(&self, mut notes: Vec<Note>) -> Vec<Note> {
        for note in &mut notes {
            for attachment in &mut note.attachments {
                attachment.path = self
                    .today_attachment_path(attachment)
                    .map(|path| path.to_string_lossy().into_owned());
            }
        }
        notes
    }
}

/// Copies each file into `root/relative`, each under a unique safe name.
fn copy_files(
    root: &Path,
    relative: &Path,
    files: &[PathBuf],
) -> (Vec<NoteAttachment>, Vec<String>) {
    let dir = root.join(relative);
    let mut attachments = Vec::new();
    let mut skipped = Vec::new();
    for source in files {
        let name = display_name(source);
        let copied = check_file(source)
            .and_then(|_| std::fs::read(source).map_err(|_| format!("{name} could not be read.")))
            .and_then(|data| {
                write_attachment_limited(&dir, &name, &data, MAX_FILE_BYTES)
                    .map(|path| (path, data.len() as u64))
                    .map_err(|_| format!("{name} could not be saved."))
            });
        match copied {
            Ok((path, size)) => {
                let leaf = path
                    .file_name()
                    .map(|leaf| leaf.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let (kind, mime, size_px) = describe(&name, &path);
                let file = relative
                    .join(leaf)
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                // Resolve like a later read does, so the path is the same
                // string either way (on Windows the copy's own path can mix
                // separators).
                let path = resolve(root, &file).unwrap_or(path);
                attachments.push(NoteAttachment {
                    file,
                    name,
                    size,
                    kind,
                    mime,
                    width: size_px.map(|(w, _)| w),
                    height: size_px.map(|(_, h)| h),
                    path: Some(path.to_string_lossy().into_owned()),
                });
            }
            Err(error) => skipped.push(error),
        }
    }
    (attachments, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::today::{AttachmentKind, NewTask};

    const NOW: u64 = 1_800_000_000_000;

    fn store_in(dir: &Path) -> Store {
        Store::open(&dir.join("monocode.db")).unwrap()
    }
    fn task(store: &Store) -> String {
        store
            .today_add(
                &NewTask {
                    title: "Ship".into(),
                    ..NewTask::default()
                },
                &Actor::User,
                NOW,
            )
            .unwrap()
            .id
    }
    fn png(path: &Path, w: u32, h: u32) {
        image::RgbaImage::new(w, h)
            .save_with_format(path, image::ImageFormat::Png)
            .unwrap();
    }

    #[test]
    fn files_are_copied_under_the_task_and_note_with_safe_names() {
        let data = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let shot = outside.path().join("my shot (1).png");
        png(&shot, 4, 3);
        let spec = outside.path().join("spec.pdf");
        std::fs::write(&spec, b"%PDF").unwrap();

        let saved = store
            .today_note_with_files(
                &id,
                "  see these  ",
                &[shot.clone(), spec.clone()],
                &Actor::User,
                NOW,
            )
            .unwrap();
        assert!(saved.skipped.is_empty());
        let note = &saved.note;
        assert_eq!(note.body, "see these");
        let [image, file] = note.attachments.as_slice() else {
            panic!("two attachments");
        };
        assert_eq!(image.name, "my shot (1).png");
        assert_eq!(image.kind, AttachmentKind::Image);
        assert_eq!(image.mime.as_deref(), Some("image/png"));
        assert_eq!((image.width, image.height), (Some(4), Some(3)));
        assert_eq!(file.kind, AttachmentKind::File);
        assert_eq!(file.size, 4);
        let prefix = format!("today-attachments/{id}/{}/", note.id);
        for attachment in [image, file] {
            assert!(attachment.file.starts_with(&prefix), "{}", attachment.file);
            let leaf = &attachment.file[prefix.len()..];
            assert!(leaf
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')));
            let stored = data.path().join(&attachment.file);
            // Compare as paths: on Windows the stored path uses `\` while the
            // portable relative `file` joined here keeps its `/` separators.
            assert_eq!(
                attachment.path.as_deref().map(std::path::Path::new),
                Some(stored.as_path())
            );
            assert!(stored.is_file());
        }
        assert_eq!(
            std::fs::read(data.path().join(&file.file)).unwrap(),
            b"%PDF"
        );

        // The original is never referenced: delete it and the note still reads.
        std::fs::remove_file(&shot).unwrap();
        let notes = store.today_notes(&id).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].attachments, note.attachments);
        let raw: String = store
            .conn
            .query_row("SELECT attachments_json FROM today_notes", [], |r| r.get(0))
            .unwrap();
        assert!(!raw.contains(outside.path().to_str().unwrap()));
        assert!(!raw.contains("\"path\""));
    }

    #[test]
    fn a_note_may_hold_only_attachments_but_not_nothing() {
        let data = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let file = data.path().join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        let saved = store
            .today_note_with_files(&id, "   ", &[file], &Actor::User, NOW)
            .unwrap();
        assert_eq!(saved.note.body, "");
        assert_eq!(saved.note.attachments.len(), 1);
        assert_eq!(
            store
                .today_note_with_files(&id, " ", &[], &Actor::User, NOW)
                .unwrap_err(),
            "note cannot be empty"
        );
    }

    #[test]
    fn a_bad_file_is_skipped_and_the_rest_go_through() {
        let data = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let good = data.path().join("good.txt");
        std::fs::write(&good, b"ok").unwrap();
        let big = data.path().join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let gone = data.path().join("gone.txt");
        let saved = store
            .today_note_with_files(
                &id,
                "",
                &[big.clone(), good, gone.clone()],
                &Actor::User,
                NOW,
            )
            .unwrap();
        assert_eq!(saved.note.attachments.len(), 1);
        assert_eq!(
            saved.skipped,
            [
                "big.bin is larger than 25 MB.",
                "gone.txt could not be read."
            ]
        );
        // Nothing usable and no text: refused, and nothing is written.
        let error = store
            .today_note_with_files(&id, "", &[big, gone], &Actor::User, NOW)
            .unwrap_err();
        assert_eq!(
            error,
            "big.bin is larger than 25 MB.\ngone.txt could not be read."
        );
        assert_eq!(store.today_notes(&id).unwrap().len(), 1);
    }

    #[test]
    fn the_split_steps_copy_without_the_store_and_commit_later() {
        let data = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let spec = outside.path().join("spec.pdf");
        std::fs::write(&spec, b"%PDF").unwrap();

        let plan = store
            .today_note_plan(&id, " hi ", 1, &Actor::User, NOW)
            .unwrap();
        // Nothing is written by the plan, and copying needs no store.
        assert!(store.today_notes(&id).unwrap().is_empty());
        let (attachments, skipped) = plan.copy(std::slice::from_ref(&spec)).unwrap();
        assert!(skipped.is_empty());
        let stored = data.path().join(&attachments[0].file);
        assert!(stored.is_file());
        assert!(store.today_notes(&id).unwrap().is_empty());
        let saved = store
            .today_note_commit(plan, attachments, skipped, &Actor::User, NOW)
            .unwrap();
        assert_eq!(saved.note.body, "hi");
        assert_eq!(store.today_notes(&id).unwrap().len(), 1);

        // A commit that cannot write takes its copies back.
        let plan = store
            .today_note_plan(&id, "again", 1, &Actor::User, NOW)
            .unwrap();
        let (attachments, skipped) = plan.copy(&[spec]).unwrap();
        let copy = data.path().join(&attachments[0].file);
        assert!(copy.is_file());
        store.conn.execute("DROP TABLE today_notes", []).unwrap();
        assert!(store
            .today_note_commit(plan, attachments, skipped, &Actor::User, NOW)
            .is_err());
        assert!(!copy.exists());
    }

    #[test]
    fn more_than_ten_files_are_refused() {
        let data = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let file = data.path().join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        let files = vec![file; MAX_PER_NOTE + 1];
        assert_eq!(
            store
                .today_note_with_files(&id, "x", &files, &Actor::User, NOW)
                .unwrap_err(),
            "A note holds at most 10 attachments"
        );
        assert!(store.today_notes(&id).unwrap().is_empty());
        assert!(!data.path().join("today-attachments").exists());
    }

    #[test]
    fn an_unknown_task_copies_nothing() {
        let data = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        task(&store);
        let file = data.path().join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        assert_eq!(
            store
                .today_note_with_files("missing", "x", &[file], &Actor::User, NOW)
                .unwrap_err(),
            NOT_FOUND
        );
        assert!(!data.path().join("today-attachments").exists());
    }

    #[test]
    fn a_memory_store_takes_text_notes_but_not_files() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let id = task(&store);
        store.today_note(&id, "plain", &Actor::User, NOW).unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            store
                .today_note_with_files(&id, "x", &[file.path().to_path_buf()], &Actor::User, NOW)
                .unwrap_err(),
            "Attachment storage is unavailable"
        );
    }

    #[test]
    fn an_old_notes_table_gains_the_column_and_keeps_its_notes() {
        let data = tempfile::tempdir().unwrap();
        let id = {
            let store = store_in(data.path());
            let id = task(&store);
            store.today_note(&id, "before", &Actor::User, NOW).unwrap();
            // Rebuild the table as an older build made it.
            store
                .conn
                .execute_batch(
                    "CREATE TABLE old_notes AS SELECT id, task_id, day, at, mono_id, body FROM today_notes;
                     DROP TABLE today_notes;
                     ALTER TABLE old_notes RENAME TO today_notes;",
                )
                .unwrap();
            assert!(
                !super::super::has_column(&store.conn, "today_notes", "attachments_json").unwrap()
            );
            id
        };
        let store = store_in(data.path());
        let notes = store.today_notes(&id).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].body, "before");
        assert!(notes[0].attachments.is_empty());
        assert!(super::super::has_column(&store.conn, "today_notes", "attachments_json").unwrap());
        // Unreadable metadata hides the files, never the note.
        store
            .conn
            .execute("UPDATE today_notes SET attachments_json = 'nope'", [])
            .unwrap();
        assert_eq!(store.today_notes(&id).unwrap()[0].body, "before");
    }

    #[test]
    fn a_mono_sees_attachment_metadata_and_the_stored_path_in_json() {
        let data = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let file = data.path().join("log.txt");
        std::fs::write(&file, b"abc").unwrap();
        store
            .today_note_with_files(&id, "", &[file], &Actor::User, NOW)
            .unwrap();
        let note = &store.today_notes(&id).unwrap()[0];
        let json = serde_json::to_value(note).unwrap();
        let listed = &json["attachments"][0];
        assert_eq!(listed["name"], "log.txt");
        assert_eq!(listed["kind"], "file");
        assert_eq!(listed["size"], 3);
        assert_eq!(
            listed["path"].as_str(),
            store
                .today_attachment_path(&note.attachments[0])
                .unwrap()
                .to_str()
        );
        // A note without files serializes as before.
        let plain = store.today_note(&id, "x", &Actor::User, NOW).unwrap();
        assert!(serde_json::to_value(plain)
            .unwrap()
            .get("attachments")
            .is_none());
    }
}
