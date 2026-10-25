//! Files the app writes so that a chat can attach them (a Contributors
//! snapshot, a pasted blob): port of `write_attachment` in the Tauri
//! `src-tauri/src/fs.rs`.

use crate::composer::MAX_EMBED_BYTES;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DIR_NAME: &str = "monocode-attachments";

/// Where attachment files go: `$TMPDIR/monocode-attachments`.
pub fn attachment_dir() -> PathBuf {
    std::env::temp_dir().join(DIR_NAME)
}

/// Writes `data` as `<pid>-<nanos>-<safe name>` in the attachment directory
/// and returns the path. The file is owner-only: a pasted screenshot can be a
/// picture of whatever was on screen.
pub fn write_attachment(name: &str, data: &[u8]) -> Result<PathBuf, String> {
    write_attachment_into(&attachment_dir(), name, data)
}

/// `write_attachment` with the directory chosen by the caller.
pub fn write_attachment_into(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf, String> {
    if data.len() as u64 > MAX_EMBED_BYTES {
        return Err(format!(
            "File is too large to attach (maximum {} MB).",
            MAX_EMBED_BYTES / 1024 / 1024
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!(
        "{}-{}-{}",
        std::process::id(),
        stamp,
        safe_attachment_name(name)
    ));
    // Keep it owner-only rather than at the umask default of 0644 in a shared /tmp.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&path)
        .and_then(|mut file| file.write_all(data))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The last path segment with anything but letters, digits, `.`, `-` and `_`
/// turned into `-`, at most 80 characters; `attachment` when nothing is left.
pub fn safe_attachment_name(name: &str) -> String {
    let leaf = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    let cleaned: String = leaf
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.').trim_matches('-');
    if trimmed.is_empty() {
        "attachment".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_bytes_round_trip_through_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_attachment_into(dir.path(), "shot.png", b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        assert!(path.starts_with(dir.path()));
    }

    #[test]
    fn the_file_is_named_pid_nanos_and_the_safe_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_attachment_into(dir.path(), "contributors-2026-10.json", b"{}").unwrap();
        let file = path.file_name().unwrap().to_str().unwrap();
        let mut parts = file.splitn(3, '-');
        assert_eq!(parts.next().unwrap(), std::process::id().to_string());
        assert!(parts.next().unwrap().chars().all(|c| c.is_ascii_digit()));
        assert_eq!(parts.next().unwrap(), "contributors-2026-10.json");
    }

    #[test]
    fn the_directory_is_created_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        let path = write_attachment_into(&nested, "x.txt", b"x").unwrap();
        assert!(path.starts_with(&nested));
    }

    #[test]
    fn two_writes_of_the_same_name_never_share_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_attachment_into(dir.path(), "same.json", b"1").unwrap();
        let second = write_attachment_into(dir.path(), "same.json", b"2").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"1");
        assert_eq!(std::fs::read(&second).unwrap(), b"2");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = write_attachment_into(dir.path(), "secret.png", b"x").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn data_above_the_embed_limit_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let data = vec![0u8; MAX_EMBED_BYTES as usize + 1];
        let error = write_attachment_into(dir.path(), "big.bin", &data).unwrap_err();
        assert_eq!(error, "File is too large to attach (maximum 20 MB).");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        // exactly the limit still goes through
        let ok = vec![0u8; MAX_EMBED_BYTES as usize];
        assert!(write_attachment_into(dir.path(), "edge.bin", &ok).is_ok());
    }

    #[test]
    fn an_unwritable_directory_is_an_error_with_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        // a directory below a regular file cannot be created
        assert!(write_attachment_into(&blocker.join("sub"), "x.txt", b"x").is_err());
    }

    #[test]
    fn safe_attachment_name_keeps_the_leaf_and_cleans_it() {
        assert_eq!(safe_attachment_name("../../secret.png"), "secret.png");
        assert_eq!(safe_attachment_name(""), "attachment");
        assert_eq!(safe_attachment_name("a b/c d?.txt"), "c-d-.txt");
        assert_eq!(safe_attachment_name("..."), "attachment");
        assert_eq!(safe_attachment_name("--x--"), "x");
        assert_eq!(safe_attachment_name("naïve.txt"), "na-ve.txt");
        assert_eq!(safe_attachment_name(&"a".repeat(200)).len(), 80);
    }

    #[test]
    fn the_directory_lives_under_the_temp_dir() {
        let dir = attachment_dir();
        assert!(dir.starts_with(std::env::temp_dir()));
        assert!(dir.ends_with("monocode-attachments"));
    }
}
