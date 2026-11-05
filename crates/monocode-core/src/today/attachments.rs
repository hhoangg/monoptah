//! Files attached to a Today note. Each file is copied into the app data
//! directory at `today-attachments/<task id>/<note id>/<unique safe name>`, so a
//! note never points at the user's original path. The copies live as long as the
//! note: editing a note away from a file, or deleting the note, removes them
//! (see `Store::today_edit_note` and `today_delete_note`).
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

/// Validates the files an agent wants to attach and returns their canonical
/// paths. Every path must be absolute and name an existing regular file within
/// [`MAX_FILE_BYTES`] (symlinks are followed, then judged by where they lead).
/// Hidden files and directories (a component starting with `.`) are refused, as
/// is anything inside the app data directory `data_dir`. `roots` limits where
/// the files may live (a session's project folders); `None` means no limit (a
/// Mono). For a session the hidden-component rule applies below the matched
/// root, so a project that itself lives in a dot folder still works. Every
/// error names the offending path.
pub fn agent_files(
    paths: &[String],
    roots: Option<&[PathBuf]>,
    data_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    if paths.len() > MAX_PER_NOTE {
        return Err(format!("A note holds at most {MAX_PER_NOTE} attachments"));
    }
    let data_dir = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    let roots: Option<Vec<PathBuf>> = roots.map(|roots| {
        roots
            .iter()
            .map(|root| root.canonicalize().unwrap_or_else(|_| root.clone()))
            .collect()
    });
    let hidden = |path: &Path| {
        path.components().any(|part| match part {
            Component::Normal(name) => name.to_string_lossy().starts_with('.'),
            _ => false,
        })
    };
    let mut out = Vec::with_capacity(paths.len());
    for raw in paths {
        let given = Path::new(raw);
        if !given.is_absolute() {
            return Err(format!("{raw} is not an absolute path."));
        }
        let canonical = given
            .canonicalize()
            .map_err(|_| format!("{raw} could not be read."))?;
        // The name the agent gave, even when it is a link to a plain file.
        if given
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            return Err(format!("{raw} is a hidden file."));
        }
        let below = match &roots {
            Some(roots) => {
                let root = roots
                    .iter()
                    .find(|root| canonical.starts_with(root))
                    .ok_or_else(|| format!("{raw} is outside the session's project folders."))?;
                canonical.strip_prefix(root).unwrap_or(&canonical)
            }
            None => canonical.as_path(),
        };
        if hidden(below) {
            return Err(format!("{raw} is a hidden file or inside a hidden folder."));
        }
        if canonical.starts_with(&data_dir) {
            return Err(format!("{raw} is inside the app's data folder."));
        }
        if !canonical.is_file() {
            return Err(format!("{raw} is not a regular file."));
        }
        check_file(&canonical).map_err(|error| format!("{raw}: {error}"))?;
        out.push(canonical);
    }
    Ok(out)
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
            edited_at: None,
            session_id: None,
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

    fn dir() -> tempfile::TempDir {
        // Not `tempdir()`: its names start with a dot, which agent_files refuses.
        tempfile::Builder::new().prefix("agent").tempdir().unwrap()
    }
    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }
    fn write(path: &Path) -> String {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
        text(path)
    }

    #[test]
    fn agent_files_accepts_a_file_inside_a_root_or_anywhere_for_a_mono() {
        let (root, data) = (dir(), dir());
        let file = write(&root.path().join("sub/report.md"));
        let roots = [root.path().to_path_buf()];
        let got = agent_files(std::slice::from_ref(&file), Some(&roots), data.path()).unwrap();
        assert_eq!(got, vec![Path::new(&file).canonicalize().unwrap()]);
        // No roots: a Mono may read from anywhere.
        let other = dir();
        let elsewhere = write(&other.path().join("a.html"));
        assert_eq!(
            agent_files(&[elsewhere], None, data.path()).unwrap().len(),
            1
        );
        assert!(agent_files(&[], None, data.path()).unwrap().is_empty());
    }

    #[test]
    fn agent_files_rejects_outside_a_root_and_names_the_path() {
        let (root, other, data) = (dir(), dir(), dir());
        let outside = write(&other.path().join("a.md"));
        let roots = [root.path().to_path_buf()];
        let error =
            agent_files(std::slice::from_ref(&outside), Some(&roots), data.path()).unwrap_err();
        assert!(error.contains(&outside), "{error}");
        assert!(error.contains("outside"), "{error}");
    }

    #[test]
    fn agent_files_rejects_hidden_files_and_hidden_folders() {
        let (root, data) = (dir(), dir());
        let roots = [root.path().to_path_buf()];
        for rel in [".env", ".git/config", "a/.secret/b.md"] {
            let file = write(&root.path().join(rel));
            let error =
                agent_files(std::slice::from_ref(&file), Some(&roots), data.path()).unwrap_err();
            assert!(error.contains(&file) && error.contains("hidden"), "{error}");
            let error = agent_files(std::slice::from_ref(&file), None, data.path()).unwrap_err();
            assert!(error.contains("hidden"), "{error}");
        }
        // `..` is resolved before the checks, so it cannot reach out of a root.
        let file = write(&root.path().join("a/b.md"));
        let tricky = text(&root.path().join("a/../a/b.md"));
        let got = agent_files(&[tricky], Some(&roots), data.path()).unwrap();
        assert_eq!(got, vec![Path::new(&file).canonicalize().unwrap()]);
        let up = text(&root.path().join("a/../../x.md"));
        assert!(agent_files(&[up], Some(&roots), data.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn agent_files_rejects_a_symlink_that_escapes_the_root() {
        let (root, other, data) = (dir(), dir(), dir());
        let target = write(&other.path().join("secret.md"));
        let link = root.path().join("link.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let roots = [root.path().to_path_buf()];
        let error = agent_files(&[text(&link)], Some(&roots), data.path()).unwrap_err();
        assert!(
            error.contains(&text(&link)) && error.contains("outside"),
            "{error}"
        );
        // A link to a hidden file is refused through its target.
        let hidden = write(&root.path().join(".key"));
        let via = root.path().join("plain.md");
        std::os::unix::fs::symlink(&hidden, &via).unwrap();
        assert!(agent_files(&[text(&via)], Some(&roots), data.path()).is_err());
    }

    #[test]
    fn agent_files_rejects_directories_missing_files_and_relative_paths() {
        let (root, data) = (dir(), dir());
        std::fs::create_dir(root.path().join("folder")).unwrap();
        let folder = text(&root.path().join("folder"));
        let error = agent_files(std::slice::from_ref(&folder), None, data.path()).unwrap_err();
        assert!(
            error.contains(&folder) && error.contains("not a regular file"),
            "{error}"
        );
        let gone = text(&root.path().join("gone.md"));
        let error = agent_files(std::slice::from_ref(&gone), None, data.path()).unwrap_err();
        assert!(error.contains(&gone), "{error}");
        let error = agent_files(&["rel/a.md".to_string()], None, data.path()).unwrap_err();
        assert!(error.contains("rel/a.md"), "{error}");
    }

    #[test]
    fn agent_files_rejects_the_data_dir_too_big_and_too_many() {
        let data = dir();
        let stored = write(&data.path().join("today-attachments/t/n/a.md"));
        let error = agent_files(std::slice::from_ref(&stored), None, data.path()).unwrap_err();
        assert!(
            error.contains(&stored) && error.contains("data folder"),
            "{error}"
        );
        let root = dir();
        let big = root.path().join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let error = agent_files(&[text(&big)], None, data.path()).unwrap_err();
        assert!(error.contains(&text(&big)), "{error}");
        let many: Vec<String> = (0..=MAX_PER_NOTE)
            .map(|i| write(&root.path().join(format!("f{i}.md"))))
            .collect();
        let error = agent_files(&many, None, data.path()).unwrap_err();
        assert!(error.contains("at most"), "{error}");
        assert_eq!(
            agent_files(&many[..MAX_PER_NOTE], None, data.path())
                .unwrap()
                .len(),
            MAX_PER_NOTE
        );
    }
}
