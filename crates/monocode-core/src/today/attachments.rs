//! Files attached to a Today note. Each file is copied into the app data
//! directory at `today-attachments/<task id>/<note id>/<unique safe name>`, so a
//! note never points at the user's original path. Notes are append-only, so the
//! copies are too: nothing here deletes them.
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// The folder under the app data directory that holds every note's files.
pub const DIR: &str = "today-attachments";
/// At most this many files on one note.
pub const MAX_PER_NOTE: usize = 10;
/// At most this many bytes per file.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// At most this many note images go into a session started from a task (see
/// [`turn_images`]); the chat's own attachment limits apply on top.
pub const MAX_TURN_IMAGES: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    /// PNG, JPEG, GIF or WebP: shown inline and sent to an agent as an image.
    Image,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteAttachment {
    /// The original file name, for display. Data, never a path.
    pub name: String,
    /// The stored copy, relative to the app data directory:
    /// `today-attachments/<task id>/<note id>/<file>`.
    pub file: String,
    pub size: u64,
    pub kind: AttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// The stored copy's absolute path. Filled when notes are read, never
    /// stored; `None` when the store has no data directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl NoteAttachment {
    pub fn is_image(&self) -> bool {
        self.kind == AttachmentKind::Image
    }
}

/// The folder for one note's files, relative to the data directory.
pub fn note_dir(task_id: &str, note_id: &str) -> PathBuf {
    Path::new(DIR)
        .join(crate::attachment::safe_attachment_name(task_id))
        .join(crate::attachment::safe_attachment_name(note_id))
}

/// `file` under `data_dir`, or `None` when `file` is not exactly
/// `today-attachments/<task>/<note>/<name>` of plain segments. Does not check
/// that the file exists.
pub fn resolve(data_dir: &Path, file: &str) -> Option<PathBuf> {
    let parts = Path::new(file)
        .components()
        .map(|part| match part {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 4 || parts[0] != DIR {
        return None;
    }
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    if !parts[1..].iter().all(|part| plain(part)) {
        return None;
    }
    Some(data_dir.join(file))
}

/// The size of `path` when it can be attached to a note: a regular file of at
/// most [`MAX_FILE_BYTES`]. The error names the file and says why.
pub fn check_file(path: &Path) -> Result<u64, String> {
    let name = display_name(path);
    let meta = path
        .metadata()
        .map_err(|_| format!("{name} could not be read."))?;
    if !meta.is_file() {
        return Err(format!("{name} is not a file."));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{name} is larger than {} MB.",
            MAX_FILE_BYTES / 1024 / 1024
        ));
    }
    Ok(meta.len())
}

/// The last segment of `path`, as shown to the user.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "attachment".into())
}

/// Kind, mime and (for images, read from the header only) dimensions of a file
/// called `name` at `path`.
pub fn describe(name: &str, path: &Path) -> (AttachmentKind, Option<String>, Option<(u32, u32)>) {
    match crate::composer::image_mime(Path::new(name)) {
        Some(mime) => (
            AttachmentKind::Image,
            Some(mime.to_owned()),
            image::ImageReader::open(path)
                .and_then(|reader| reader.with_guessed_format())
                .ok()
                .and_then(|reader| reader.into_dimensions().ok()),
        ),
        None => (AttachmentKind::File, None, None),
    }
}

/// The image copies to send with a session that mentions these notes' task:
/// newest note first, at most `max_count` (and never more than
/// [`MAX_TURN_IMAGES`]) files totalling at most `max_bytes`. Only images whose
/// stored copy still exists count; an image that does not fit is skipped and a
/// smaller later one may still go. `notes` is oldest first, as stored.
pub fn turn_images(notes: &[super::Note], max_count: usize, max_bytes: u64) -> Vec<PathBuf> {
    let max_count = max_count.min(MAX_TURN_IMAGES);
    let mut picked = Vec::new();
    let mut total = 0u64;
    for note in notes.iter().rev() {
        for attachment in note.attachments.iter().filter(|a| a.is_image()) {
            if picked.len() >= max_count {
                return picked;
            }
            let Some(path) = attachment.path.as_deref().map(PathBuf::from) else {
                continue;
            };
            let Ok(meta) = path.metadata() else {
                continue;
            };
            if !meta.is_file() || total + meta.len() > max_bytes {
                continue;
            }
            total += meta.len();
            picked.push(path);
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::today::Note;

    #[test]
    fn a_note_folder_is_task_then_note_under_the_root() {
        assert_eq!(
            note_dir("task-1", "note-2"),
            Path::new("today-attachments/task-1/note-2")
        );
        // An id is never a path.
        assert_eq!(note_dir("../x", "a/b"), Path::new("today-attachments/x/b"));
    }

    #[test]
    fn resolve_accepts_only_a_stored_shape() {
        let root = Path::new("/data");
        assert_eq!(
            resolve(root, "today-attachments/task-1/note-2/1-2-shot.png"),
            Some(PathBuf::from(
                "/data/today-attachments/task-1/note-2/1-2-shot.png"
            ))
        );
        for bad in [
            "",
            "/etc/passwd",
            "today-attachments/../x/y/z",
            "today-attachments/task-1/note-2",
            "today-attachments/task-1/note-2/a/b",
            "other/task-1/note-2/a.png",
            "today-attachments/task 1/note-2/a.png",
        ] {
            assert_eq!(resolve(root, bad), None, "{bad}");
        }
    }

    #[test]
    fn check_file_refuses_folders_missing_and_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("ok.txt");
        std::fs::write(&ok, b"hello").unwrap();
        assert_eq!(check_file(&ok), Ok(5));
        assert_eq!(
            check_file(dir.path()).unwrap_err(),
            format!("{} is not a file.", display_name(dir.path()))
        );
        assert_eq!(
            check_file(&dir.path().join("gone.txt")).unwrap_err(),
            "gone.txt could not be read."
        );
        let big = dir.path().join("big.bin");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert_eq!(
            check_file(&big).unwrap_err(),
            "big.bin is larger than 25 MB."
        );
        file.set_len(MAX_FILE_BYTES).unwrap();
        assert_eq!(check_file(&big), Ok(MAX_FILE_BYTES));
    }

    #[test]
    fn describe_reads_image_dimensions_from_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("x");
        image::RgbaImage::new(3, 2)
            .save_with_format(&png, image::ImageFormat::Png)
            .unwrap();
        assert_eq!(
            describe("Shot.PNG", &png),
            (
                AttachmentKind::Image,
                Some("image/png".into()),
                Some((3, 2))
            )
        );
        assert_eq!(
            describe("spec.pdf", &png),
            (AttachmentKind::File, None, None)
        );
        // A broken image is still an image, just without dimensions.
        let broken = dir.path().join("broken.png");
        std::fs::write(&broken, b"nope").unwrap();
        assert_eq!(
            describe("broken.png", &broken),
            (AttachmentKind::Image, Some("image/png".into()), None)
        );
    }

    fn attachment(path: &Path, kind: AttachmentKind) -> NoteAttachment {
        NoteAttachment {
            name: display_name(path),
            file: String::new(),
            size: 0,
            kind,
            mime: None,
            width: None,
            height: None,
            path: Some(path.to_string_lossy().into_owned()),
        }
    }
    fn note(at: u64, attachments: Vec<NoteAttachment>) -> Note {
        Note {
            id: format!("note-{at}"),
            task_id: "task-1".into(),
            day: "2026-11-04".into(),
            at,
            mono_id: None,
            body: String::new(),
            attachments,
        }
    }

    #[test]
    fn turn_images_take_the_newest_existing_images_within_the_limits() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, len: usize| {
            let path = dir.path().join(name);
            std::fs::write(&path, vec![0u8; len]).unwrap();
            path
        };
        let old = file("old.png", 10);
        let doc = file("doc.pdf", 10);
        let new = file("new.png", 10);
        let big = file("big.png", 100);
        let small = file("small.png", 5);
        let missing = dir.path().join("missing.png");
        let notes = vec![
            note(1, vec![attachment(&old, AttachmentKind::Image)]),
            note(
                2,
                vec![
                    attachment(&doc, AttachmentKind::File),
                    attachment(&missing, AttachmentKind::Image),
                    attachment(&new, AttachmentKind::Image),
                    attachment(&big, AttachmentKind::Image),
                    attachment(&small, AttachmentKind::Image),
                ],
            ),
        ];
        // Newest note first, files and missing copies skipped, a too big one
        // passed over for a smaller one.
        assert_eq!(
            turn_images(&notes, 10, 30),
            vec![new.clone(), small.clone(), old.clone()]
        );
        assert_eq!(
            turn_images(&notes, 2, 1_000),
            vec![new.clone(), big.clone()]
        );
        assert!(turn_images(&notes, 0, 1_000).is_empty());
        assert!(turn_images(&notes, 10, 0).is_empty());
        // Never more than MAX_TURN_IMAGES, whatever the chat allows.
        let many: Vec<_> = (0..MAX_TURN_IMAGES + 3)
            .map(|i| note(i as u64, vec![attachment(&small, AttachmentKind::Image)]))
            .collect();
        assert_eq!(turn_images(&many, 100, u64::MAX).len(), MAX_TURN_IMAGES);
    }
}
