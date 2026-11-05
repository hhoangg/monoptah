//! Editing and deleting a Today note. A note is written once; an edit
//! keeps the note's id, day, time and author and stamps `edited_at`; a delete
//! removes the row and the note's stored files.
use super::{actor_sees, load_task, note_from_row, sql, Store, NOTE_COLUMNS, NOTE_MAX, NOT_FOUND};
use crate::today::attachments::{note_dir, resolve};
use crate::today::{Actor, Note, NoteAttachment};
use rusqlite::{params, Connection, OptionalExtension};

const NOTE_NOT_FOUND: &str = "Note not found";
const NOT_YOURS: &str = "A Mono can only edit or delete its own notes";
const NOT_YOUR_SESSION: &str = "A session can only edit or delete its own notes";

/// The note `note_id` of the task `task_id_or_key`, if `actor` may change it.
/// A note of another task reads as not found.
fn writable_note(
    conn: &Connection,
    task_id_or_key: &str,
    note_id: &str,
    actor: &Actor,
) -> Result<Note, String> {
    let task = load_task(conn, task_id_or_key)?
        .filter(|task| actor_sees(conn, actor, task))
        .ok_or_else(|| NOT_FOUND.to_string())?;
    let note = conn
        .query_row(
            &format!("SELECT {NOTE_COLUMNS} FROM today_notes WHERE id = ?1 AND task_id = ?2"),
            params![note_id, task.id],
            note_from_row,
        )
        .optional()
        .map_err(sql)?
        .ok_or_else(|| NOTE_NOT_FOUND.to_string())?;
    match actor {
        Actor::User => {}
        // Not a session's note either: those belong to the session alone.
        Actor::Mono(caller) => {
            if note.mono_id.as_deref() != Some(caller.id.as_str()) || note.session_id.is_some() {
                return Err(NOT_YOURS.to_string());
            }
        }
        Actor::Session(id) => {
            if note.session_id != Some(*id) {
                return Err(NOT_YOUR_SESSION.to_string());
            }
        }
    }
    Ok(note)
}

impl Store {
    /// Replaces a note's body and drops the attachments whose `file` is in
    /// `remove_attachments`; the rest stay as they are. The note keeps its id,
    /// day, time and author. `edited_at` is set only when something changed, and
    /// a no-op edit writes nothing. The stored copies of dropped files are
    /// removed after the database write succeeded. A Mono may change only its
    /// own notes, a session only its own; the user any.
    pub fn today_edit_note(
        &self,
        task_id_or_key: &str,
        note_id: &str,
        body: &str,
        remove_attachments: &[String],
        actor: &Actor,
        now: u64,
    ) -> Result<Note, String> {
        self.today_writable()?;
        let body = body.trim();
        if body.chars().count() > NOTE_MAX {
            return Err(format!("A note is at most {NOTE_MAX} characters"));
        }
        self.today_roll(now)?;
        self.today_ready()?;
        let (note, removed) = self.today_tx(|conn| {
            let mut note = writable_note(conn, task_id_or_key, note_id, actor)?;
            if remove_attachments
                .iter()
                .any(|file| !note.attachments.iter().any(|a| &a.file == file))
            {
                return Err("Attachment not found".to_string());
            }
            let (removed, kept): (Vec<NoteAttachment>, Vec<NoteAttachment>) = note
                .attachments
                .drain(..)
                .partition(|a| remove_attachments.contains(&a.file));
            if body.is_empty() && kept.is_empty() {
                return Err("note cannot be empty".to_string());
            }
            if body == note.body && removed.is_empty() {
                note.attachments = kept;
                return Ok((note, removed));
            }
            let json = (!kept.is_empty())
                .then(|| serde_json::to_string(&kept))
                .transpose()
                .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE today_notes SET body = ?2, attachments_json = ?3, edited_at = ?4
                 WHERE id = ?1",
                params![note.id, body, json, now.min(i64::MAX as u64) as i64],
            )
            .map_err(sql)?;
            note.body = body.to_string();
            note.attachments = kept;
            note.edited_at = Some(now);
            Ok((note, removed))
        })?;
        if let Some(root) = self.data_dir() {
            for attachment in &removed {
                if let Some(path) = resolve(root, &attachment.file) {
                    let _ = std::fs::remove_file(path);
                }
            }
            if !removed.is_empty() {
                // Only succeeds when the folder is empty.
                let _ = std::fs::remove_dir(root.join(note_dir(&note.task_id, &note.id)));
            }
        }
        Ok(self
            .with_attachment_paths(vec![note])
            .pop()
            .expect("one note"))
    }

    /// Deletes a note and, after the row is gone, its stored files. A Mono may
    /// delete only its own notes, a session only its own; the user any.
    pub fn today_delete_note(
        &self,
        task_id_or_key: &str,
        note_id: &str,
        actor: &Actor,
        now: u64,
    ) -> Result<(), String> {
        self.today_writable()?;
        self.today_roll(now)?;
        self.today_ready()?;
        let note = self.today_tx(|conn| {
            let note = writable_note(conn, task_id_or_key, note_id, actor)?;
            conn.execute("DELETE FROM today_notes WHERE id = ?1", [&note.id])
                .map_err(sql)?;
            Ok(note)
        })?;
        if let Some(root) = self.data_dir() {
            let _ = std::fs::remove_dir_all(root.join(note_dir(&note.task_id, &note.id)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::today::{MonoCaller, NewTask};
    use std::path::Path;

    const NOW: u64 = 1_800_000_000_000;

    fn store_in(dir: &Path) -> Store {
        Store::open(&dir.join("monocode.db")).unwrap()
    }
    fn task_titled(store: &Store, title: &str) -> String {
        store
            .today_add(
                &NewTask {
                    title: title.into(),
                    ..NewTask::default()
                },
                &Actor::User,
                NOW,
            )
            .unwrap()
            .id
    }
    fn task(store: &Store) -> String {
        task_titled(store, "Ship")
    }
    fn mono(id: &str) -> Actor {
        Actor::Mono(MonoCaller {
            id: id.into(),
            ..MonoCaller::default()
        })
    }
    fn files(dir: &Path, names: &[&str]) -> Vec<std::path::PathBuf> {
        names
            .iter()
            .map(|name| {
                let path = dir.join(name);
                std::fs::write(&path, name.as_bytes()).unwrap();
                path
            })
            .collect()
    }
    fn edit(store: &Store, task: &str, note: &str, body: &str) -> Result<Note, String> {
        store.today_edit_note(task, note, body, &[], &Actor::User, NOW + 5)
    }

    #[test]
    fn an_edit_changes_the_body_and_keeps_who_when_and_where() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let id = task(&store);
        let original = store
            .today_note(&id, "first", &mono("m1"), NOW + 1)
            .unwrap();
        let edited = store
            .today_edit_note(&id, &original.id, "  second  ", &[], &Actor::User, NOW + 9)
            .unwrap();
        assert_eq!(edited.body, "second");
        assert_eq!(edited.edited_at, Some(NOW + 9));
        assert_eq!(
            (&edited.id, &edited.task_id, &edited.day, edited.at),
            (&original.id, &original.task_id, &original.day, original.at)
        );
        assert_eq!(edited.mono_id.as_deref(), Some("m1"));
        let read = store.today_notes(&id).unwrap();
        assert_eq!(read, std::slice::from_ref(&edited));
        let json = serde_json::to_value(&edited).unwrap();
        assert_eq!(json["editedAt"], NOW + 9);
        // The task key works like the id.
        let key = store.today_task(&id).unwrap().unwrap().key;
        assert_eq!(
            edit(&store, &key, &original.id, "third").unwrap().body,
            "third"
        );
    }

    #[test]
    fn an_edit_that_changes_nothing_writes_nothing() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let id = task(&store);
        let note = store.today_note(&id, "same", &Actor::User, NOW).unwrap();
        let again = edit(&store, &id, &note.id, "  same ").unwrap();
        assert_eq!(again, note);
        assert_eq!(again.edited_at, None);
        assert_eq!(store.today_notes(&id).unwrap()[0].edited_at, None);
        assert!(serde_json::to_value(&again)
            .unwrap()
            .get("editedAt")
            .is_none());
    }

    #[test]
    fn an_edit_may_drop_one_attachment_and_deletes_only_that_file() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let picked = files(src.path(), &["a.txt", "b.txt", "c.txt"]);
        let note = store
            .today_note_with_files(&id, "files", &picked, &Actor::User, NOW)
            .unwrap()
            .note;
        let [a, b, c] = note.attachments.as_slice() else {
            panic!("three");
        };
        let stored = |attachment: &crate::today::NoteAttachment| data.path().join(&attachment.file);
        let edited = store
            .today_edit_note(
                &id,
                &note.id,
                "files",
                std::slice::from_ref(&b.file),
                &Actor::User,
                NOW + 1,
            )
            .unwrap();
        assert_eq!(edited.edited_at, Some(NOW + 1));
        assert_eq!(
            edited
                .attachments
                .iter()
                .map(|x| &x.file)
                .collect::<Vec<_>>(),
            [&a.file, &c.file]
        );
        assert!(edited.attachments.iter().all(|x| x.path.is_some()));
        assert!(stored(a).is_file() && stored(c).is_file());
        assert!(!stored(b).exists());
        assert_eq!(store.today_notes(&id).unwrap()[0].attachments.len(), 2);
    }

    #[test]
    fn an_edit_that_drops_every_attachment_removes_the_note_folder() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let picked = files(src.path(), &["a.txt", "b.txt"]);
        let note = store
            .today_note_with_files(&id, "keep text", &picked, &Actor::User, NOW)
            .unwrap()
            .note;
        let folder = data.path().join(note_dir(&id, &note.id));
        assert!(folder.is_dir());
        let all: Vec<String> = note.attachments.iter().map(|a| a.file.clone()).collect();
        let edited = store
            .today_edit_note(&id, &note.id, "keep text", &all, &Actor::User, NOW + 1)
            .unwrap();
        assert!(edited.attachments.is_empty());
        assert_eq!(edited.body, "keep text");
        assert!(!folder.exists());
    }

    #[test]
    fn an_empty_body_needs_an_attachment_left() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let picked = files(src.path(), &["a.txt", "b.txt"]);
        let note = store
            .today_note_with_files(&id, "text", &picked, &Actor::User, NOW)
            .unwrap()
            .note;
        let [a, b] = note.attachments.as_slice() else {
            panic!("two");
        };
        // Clearing the text is fine while a file stays.
        let cleared = store
            .today_edit_note(
                &id,
                &note.id,
                " ",
                std::slice::from_ref(&a.file),
                &Actor::User,
                NOW + 1,
            )
            .unwrap();
        assert_eq!(cleared.body, "");
        // Nothing would be left: refused, and nothing is deleted.
        let error = store
            .today_edit_note(
                &id,
                &note.id,
                "",
                std::slice::from_ref(&b.file),
                &Actor::User,
                NOW + 2,
            )
            .unwrap_err();
        assert_eq!(error, "note cannot be empty");
        assert!(data.path().join(&b.file).is_file());
        assert_eq!(store.today_notes(&id).unwrap()[0].attachments.len(), 1);
        let plain = store.today_note(&id, "plain", &Actor::User, NOW).unwrap();
        assert_eq!(
            edit(&store, &id, &plain.id, "   ").unwrap_err(),
            "note cannot be empty"
        );
    }

    #[test]
    fn a_too_long_body_or_an_unknown_file_is_refused_without_writing() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let note = store
            .today_note_with_files(
                &id,
                "keep",
                &files(src.path(), &["a.txt"]),
                &Actor::User,
                NOW,
            )
            .unwrap()
            .note;
        assert!(edit(&store, &id, &note.id, &"x".repeat(5_001))
            .unwrap_err()
            .contains("5000"));
        assert!(edit(&store, &id, &note.id, &"x".repeat(5_000)).is_ok());
        let error = store
            .today_edit_note(
                &id,
                &note.id,
                "other",
                &["today-attachments/x/y/z.txt".into()],
                &Actor::User,
                NOW + 1,
            )
            .unwrap_err();
        assert_eq!(error, "Attachment not found");
        let read = &store.today_notes(&id).unwrap()[0];
        assert_eq!(read.body, "x".repeat(5_000));
        assert_eq!(read.attachments.len(), 1);
        assert!(data.path().join(&note.attachments[0].file).is_file());
    }

    #[test]
    fn a_delete_removes_the_row_and_the_note_folder_only() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let store = store_in(data.path());
        let id = task(&store);
        let picked = files(src.path(), &["a.txt"]);
        let gone = store
            .today_note_with_files(&id, "gone", &picked, &Actor::User, NOW)
            .unwrap()
            .note;
        let kept = store
            .today_note_with_files(&id, "kept", &picked, &Actor::User, NOW + 1)
            .unwrap()
            .note;
        store
            .today_delete_note(&id, &gone.id, &Actor::User, NOW + 2)
            .unwrap();
        let notes = store.today_notes(&id).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, kept.id);
        assert!(!data.path().join(&gone.attachments[0].file).exists());
        assert!(!data
            .path()
            .join(crate::today::attachments::note_dir(&id, &gone.id))
            .exists());
        assert!(data.path().join(&kept.attachments[0].file).is_file());
        assert_eq!(
            store
                .today_delete_note(&id, &gone.id, &Actor::User, NOW + 3)
                .unwrap_err(),
            "Note not found"
        );
    }

    #[test]
    fn a_memory_store_deletes_the_row() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let id = task(&store);
        let note = store.today_note(&id, "x", &Actor::User, NOW).unwrap();
        store
            .today_delete_note(&id, &note.id, &Actor::User, NOW)
            .unwrap();
        assert!(store.today_notes(&id).unwrap().is_empty());
    }

    #[test]
    fn an_unknown_task_or_note_or_a_note_of_another_task_is_not_found() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let one = task_titled(&store, "One");
        let two = task_titled(&store, "Two");
        let note = store.today_note(&one, "mine", &Actor::User, NOW).unwrap();
        assert_eq!(
            edit(&store, "missing", &note.id, "x").unwrap_err(),
            NOT_FOUND
        );
        assert_eq!(
            edit(&store, &one, "missing", "x").unwrap_err(),
            "Note not found"
        );
        assert_eq!(
            edit(&store, &two, &note.id, "x").unwrap_err(),
            "Note not found"
        );
        assert_eq!(
            store
                .today_delete_note("missing", &note.id, &Actor::User, NOW)
                .unwrap_err(),
            NOT_FOUND
        );
        assert_eq!(
            store
                .today_delete_note(&two, &note.id, &Actor::User, NOW)
                .unwrap_err(),
            "Note not found"
        );
        assert_eq!(store.today_notes(&one).unwrap()[0].body, "mine");
    }

    #[test]
    fn a_mono_changes_only_its_own_notes_and_the_user_any() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let id = task(&store);
        let users = store.today_note(&id, "user", &Actor::User, NOW).unwrap();
        let theirs = store.today_note(&id, "m2", &mono("m2"), NOW).unwrap();
        let mine = store.today_note(&id, "m1", &mono("m1"), NOW).unwrap();
        let m1 = mono("m1");
        for other in [&users, &theirs] {
            let error = store
                .today_edit_note(&id, &other.id, "hijack", &[], &m1, NOW + 1)
                .unwrap_err();
            assert_eq!(error, "A Mono can only edit or delete its own notes");
            let error = store
                .today_delete_note(&id, &other.id, &m1, NOW + 1)
                .unwrap_err();
            assert_eq!(error, "A Mono can only edit or delete its own notes");
        }
        assert_eq!(store.today_notes(&id).unwrap().len(), 3);
        let edited = store
            .today_edit_note(&id, &mine.id, "m1 again", &[], &m1, NOW + 2)
            .unwrap();
        assert_eq!(edited.body, "m1 again");
        store
            .today_delete_note(&id, &mine.id, &m1, NOW + 3)
            .unwrap();
        // The user may change a Mono's note.
        assert!(edit(&store, &id, &theirs.id, "fixed").is_ok());
        store
            .today_delete_note(&id, &theirs.id, &Actor::User, NOW)
            .unwrap();
        assert_eq!(store.today_notes(&id).unwrap().len(), 1);
    }

    #[test]
    fn a_mono_cannot_reach_a_task_it_cannot_see() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        // A synced task with no project belongs to the Mono that synced it.
        let owner = MonoCaller {
            id: "owner".into(),
            ..MonoCaller::default()
        };
        store
            .today_sync(
                &crate::today::SyncRequest {
                    source: "gh".into(),
                    items: Some(vec![crate::today::SyncItem {
                        key: "gh:1".into(),
                        title: "Theirs".into(),
                        ..Default::default()
                    }]),
                    ..Default::default()
                },
                &owner,
                NOW,
            )
            .unwrap();
        let id = store.today_task("gh:1").unwrap().unwrap().id;
        let note = store
            .today_note(&id, "n", &Actor::Mono(owner), NOW)
            .unwrap();
        let stranger = mono("other");
        assert_eq!(
            store
                .today_edit_note(&id, &note.id, "x", &[], &stranger, NOW)
                .unwrap_err(),
            NOT_FOUND
        );
        assert_eq!(
            store
                .today_delete_note(&id, &note.id, &stranger, NOW)
                .unwrap_err(),
            NOT_FOUND
        );
        assert_eq!(store.today_notes(&id).unwrap()[0].body, "n");
    }

    #[test]
    fn an_old_notes_table_gains_the_edit_column_and_keeps_its_notes() {
        let data = tempfile::tempdir().unwrap();
        let (id, note_id) = {
            let store = store_in(data.path());
            let id = task(&store);
            let note = store.today_note(&id, "before", &Actor::User, NOW).unwrap();
            store
                .conn
                .execute_batch(
                    "CREATE TABLE old_notes AS SELECT id, task_id, day, at, mono_id, body, attachments_json FROM today_notes;
                     DROP TABLE today_notes;
                     ALTER TABLE old_notes RENAME TO today_notes;",
                )
                .unwrap();
            assert!(!super::super::has_column(&store.conn, "today_notes", "edited_at").unwrap());
            (id, note.id)
        };
        let store = store_in(data.path());
        let notes = store.today_notes(&id).unwrap();
        assert_eq!(notes[0].body, "before");
        assert_eq!(notes[0].edited_at, None);
        assert!(super::super::has_column(&store.conn, "today_notes", "edited_at").unwrap());
        let edited = edit(&store, &id, &note_id, "after").unwrap();
        assert_eq!(edited.edited_at, Some(NOW + 5));
    }

    #[test]
    fn a_session_note_stores_its_session_and_no_mono() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let id = task(&store);
        store.today_link_session(7, &id, NOW).unwrap();
        let note = store
            .today_note(&id, "from a session", &Actor::Session(7), NOW)
            .unwrap();
        assert_eq!((note.session_id, note.mono_id.as_deref()), (Some(7), None));
        let read = &store.today_notes(&id).unwrap()[0];
        assert_eq!(read, &note);
        let json = serde_json::to_value(read).unwrap();
        assert_eq!(json["sessionId"], 7);
        let user = store.today_note(&id, "mine", &Actor::User, NOW).unwrap();
        assert!(serde_json::to_value(&user)
            .unwrap()
            .get("sessionId")
            .is_none());
        assert_eq!(user.session_id, None);
    }

    #[test]
    fn a_session_sees_only_its_linked_task() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let (one, two) = (task_titled(&store, "One"), task_titled(&store, "Two"));
        store.today_link_session(7, &one, NOW).unwrap();
        let session = Actor::Session(7);
        assert!(store.today_note(&one, "ok", &session, NOW).is_ok());
        assert_eq!(
            store.today_note(&two, "no", &session, NOW).unwrap_err(),
            "Task not found"
        );
        let unlinked = Actor::Session(9);
        assert!(store.today_note(&one, "no", &unlinked, NOW).is_err());
    }

    #[test]
    fn sessions_and_monos_and_the_user_edit_by_ownership() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let id = task(&store);
        store.today_link_session(7, &id, NOW).unwrap();
        store.today_link_session(8, &id, NOW).unwrap();
        let session = Actor::Session(7);
        let theirs = store.today_note(&id, "s", &session, NOW).unwrap();
        let users = store.today_note(&id, "u", &Actor::User, NOW).unwrap();
        let monos = store.today_note(&id, "m", &mono("m1"), NOW).unwrap();
        let change = |actor: &Actor, note: &Note| {
            store.today_edit_note(&id, &note.id, "changed", &[], actor, NOW + 1)
        };
        assert!(change(&session, &theirs).is_ok());
        assert!(change(&session, &users).is_err());
        assert!(change(&session, &monos).is_err());
        assert!(change(&Actor::Session(8), &theirs).is_err());
        assert!(change(&mono("m1"), &theirs).is_err());
        assert!(change(&Actor::User, &theirs).is_ok());
        assert!(store
            .today_delete_note(&id, &users.id, &session, NOW)
            .is_err());
        assert!(store
            .today_delete_note(&id, &theirs.id, &mono("m1"), NOW)
            .is_err());
        assert!(store
            .today_delete_note(&id, &theirs.id, &session, NOW)
            .is_ok());
    }

    #[test]
    fn an_old_notes_table_gains_the_session_column_and_keeps_its_notes() {
        let data = tempfile::tempdir().unwrap();
        let (id, note_id) = {
            let store = store_in(data.path());
            let id = task(&store);
            let note = store.today_note(&id, "before", &Actor::User, NOW).unwrap();
            store
                .conn
                .execute_batch(
                    "CREATE TABLE old_notes AS SELECT id, task_id, day, at, mono_id, body, attachments_json, edited_at FROM today_notes;
                     DROP TABLE today_notes;
                     ALTER TABLE old_notes RENAME TO today_notes;",
                )
                .unwrap();
            assert!(!super::super::has_column(&store.conn, "today_notes", "session_id").unwrap());
            (id, note.id)
        };
        let store = store_in(data.path());
        let notes = store.today_notes(&id).unwrap();
        assert_eq!(
            (notes[0].body.as_str(), notes[0].session_id),
            ("before", None)
        );
        assert!(super::super::has_column(&store.conn, "today_notes", "session_id").unwrap());
        store.today_link_session(3, &id, NOW).unwrap();
        let written = store
            .today_note(&id, "after", &Actor::Session(3), NOW + 1)
            .unwrap();
        assert_eq!(store.today_notes(&id).unwrap()[1].session_id, Some(3));
        assert!(edit(&store, &id, &note_id, "kept").is_ok());
        assert_eq!(written.session_id, Some(3));
    }
}
