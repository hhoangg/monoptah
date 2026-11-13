//! Import of an emoji pack from an untrusted zip file. See "Zip import security" in
//! `docs/native/mono-emoji.md`.
//!
//! An entry path is never used as a filesystem destination. Every kept file is written as
//! `<first 16 hex of sha256>.<ext>` directly inside a fresh `.staging-<uuid>` folder.

use super::Format;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_zip_bytes: u64,
    pub max_entries: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_dimension: u32,
    pub max_frames: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_zip_bytes: 200 * 1024 * 1024,
            max_entries: 500,
            max_file_bytes: 10 * 1024 * 1024,
            max_total_bytes: 200 * 1024 * 1024,
            max_dimension: 1024,
            max_frames: 300,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    UnsafePath,
    Symlink,
    Hidden,
    MacMetadata,
    NestedArchive,
    Encrypted,
    NotImage,
    TooLarge,
    BadImage,
    TooManyPixels,
    TooManyFrames,
    Duplicate,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            SkipReason::UnsafePath => "unsafe path",
            SkipReason::Symlink => "symbolic link",
            SkipReason::Hidden => "hidden file",
            SkipReason::MacMetadata => "macOS metadata",
            SkipReason::NestedArchive => "nested archive",
            SkipReason::Encrypted => "encrypted",
            SkipReason::NotImage => "not a PNG, JPG, GIF or WebP image",
            SkipReason::TooLarge => "file too large",
            SkipReason::BadImage => "damaged image",
            SkipReason::TooManyPixels => "image too large",
            SkipReason::TooManyFrames => "too many frames",
            SkipReason::Duplicate => "duplicate",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub entry: String,
    pub reason: SkipReason,
}

#[derive(Clone, Debug)]
pub struct StagedImage {
    pub hash: String,
    pub file: String,
    pub name: String,
    pub format: Format,
    pub animated: bool,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

#[derive(Debug)]
pub struct Staged {
    pub dir: PathBuf,
    pub source_name: String,
    pub entries: usize,
    pub images: Vec<StagedImage>,
    pub skipped: Vec<Skipped>,
}

const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "jar", "cbz",
];

/// Detects the format from magic bytes only.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::Webp)
    } else {
        None
    }
}

fn is_archive_magic(b: &[u8]) -> bool {
    b.starts_with(b"PK\x03\x04")
        || b.starts_with(b"PK\x05\x06")
        || b.starts_with(b"Rar!\x1a\x07")
        || b.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C])
        || b.starts_with(&[0x1F, 0x8B])
        || (b.len() >= 262 && &b[257..262] == b"ustar")
        || b.starts_with(b"BZh")
        || b.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00])
        || b.starts_with(&[0x28, 0xB5, 0x2F, 0xFD])
}

/// A display name for an entry or a zip file: last path component, extension removed,
/// numeric emoji.gg prefix removed, only letters, digits, space, `-`, `_` and `.`, at most
/// 64 chars. Falls back to "emoji".
pub fn sanitize_name(entry_name: &str) -> String {
    let last = entry_name.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = match last.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => last,
    };
    let digits = stem.chars().take_while(|c| c.is_ascii_digit()).count();
    let stem = if digits > 0 && stem[digits..].starts_with('-') && stem.len() > digits + 1 {
        &stem[digits + 1..]
    } else {
        stem
    };
    let kept: String = stem
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect();
    let collapsed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(64).collect();
    let out = truncated.trim().to_string();
    if out.is_empty() {
        "emoji".to_string()
    } else {
        out
    }
}

fn source_name_of(zip_path: &Path) -> String {
    let file = zip_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = sanitize_name(&file);
    let stripped = name
        .strip_suffix("-emojigg-pack")
        .map(str::trim)
        .unwrap_or(&name);
    if stripped.is_empty() {
        "emoji".to_string()
    } else {
        stripped.to_string()
    }
}

fn parts(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw.split(|b| *b == b'/' || *b == b'\\')
}

/// True when the raw entry name could escape a folder or name an absolute location.
fn is_unsafe_path(raw: &[u8]) -> bool {
    if raw.contains(&0) {
        return true;
    }
    if matches!(raw.first(), Some(b'/') | Some(b'\\')) {
        return true;
    }
    if raw.len() >= 2 && raw[0].is_ascii_alphabetic() && raw[1] == b':' {
        return true;
    }
    parts(raw).any(|part| part == b"..")
}

fn has_hidden_component(raw: &[u8]) -> bool {
    parts(raw).any(|part| part.first() == Some(&b'.'))
}

fn has_macosx_component(raw: &[u8]) -> bool {
    parts(raw).any(|part| part == b"__MACOSX")
}

fn has_archive_extension(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => ARCHIVE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

struct Inspected {
    width: u32,
    height: u32,
    animated: bool,
}

fn inspect(bytes: &[u8], format: Format, limits: &Limits) -> Result<Inspected, SkipReason> {
    use image::AnimationDecoder;
    let image_format = match format {
        Format::Png => image::ImageFormat::Png,
        Format::Jpeg => image::ImageFormat::Jpeg,
        Format::Gif => image::ImageFormat::Gif,
        Format::Webp => image::ImageFormat::WebP,
    };
    let (width, height) = image::ImageReader::with_format(Cursor::new(bytes), image_format)
        .into_dimensions()
        .map_err(|_| SkipReason::BadImage)?;
    if width == 0 || height == 0 {
        return Err(SkipReason::BadImage);
    }
    if width > limits.max_dimension || height > limits.max_dimension {
        return Err(SkipReason::TooManyPixels);
    }
    // Frames are iterated, never collected, and counting stops at the cap plus one.
    let count_frames = |frames: image::Frames<'_>| -> Result<usize, SkipReason> {
        let mut n = 0usize;
        for frame in frames {
            frame.map_err(|_| SkipReason::BadImage)?;
            n += 1;
            if n > limits.max_frames {
                return Err(SkipReason::TooManyFrames);
            }
        }
        Ok(n)
    };
    let animated = match format {
        Format::Gif => {
            let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                .map_err(|_| SkipReason::BadImage)?;
            count_frames(decoder.into_frames())? > 1
        }
        Format::Webp => {
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|_| SkipReason::BadImage)?;
            if decoder.has_animation() {
                count_frames(decoder.into_frames())?;
                true
            } else {
                false
            }
        }
        _ => false,
    };
    Ok(Inspected {
        width,
        height,
        animated,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads the zip, validates every entry and writes the images that pass into a new
/// `.staging-<uuid>` folder inside `emoji_dir`. Any error removes the staging folder.
pub fn stage(
    zip_path: &Path,
    emoji_dir: &Path,
    existing_hashes: &HashSet<String>,
    limits: &Limits,
    cancel: &AtomicBool,
    progress: impl FnMut(usize, usize),
) -> Result<Staged, String> {
    let size = std::fs::metadata(zip_path)
        .map_err(|e| format!("Cannot read the zip file: {e}"))?
        .len();
    if size > limits.max_zip_bytes {
        return Err("The zip file is too large to import".to_string());
    }
    let file =
        std::fs::File::open(zip_path).map_err(|e| format!("Cannot open the zip file: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("This is not a valid zip file: {e}"))?;
    if archive.len() > limits.max_entries {
        return Err(format!(
            "The zip has too many files ({}, the limit is {})",
            archive.len(),
            limits.max_entries
        ));
    }
    std::fs::create_dir_all(emoji_dir)
        .map_err(|e| format!("Cannot create the emoji folder: {e}"))?;
    let dir = emoji_dir.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).map_err(|e| format!("Cannot create the staging folder: {e}"))?;
    let result = stage_into(
        &mut archive,
        &dir,
        zip_path,
        existing_hashes,
        limits,
        cancel,
        progress,
    );
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

fn stage_into(
    archive: &mut zip::ZipArchive<std::fs::File>,
    dir: &Path,
    zip_path: &Path,
    existing_hashes: &HashSet<String>,
    limits: &Limits,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Result<Staged, String> {
    let total = archive.len();
    let mut images: Vec<StagedImage> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut total_read: u64 = 0;
    for index in 0..total {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        let outcome = stage_entry(
            archive,
            index,
            dir,
            existing_hashes,
            limits,
            &mut seen,
            &mut total_read,
        )?;
        match outcome {
            Outcome::Ignore => {}
            Outcome::Skip(entry, reason) => skipped.push(Skipped { entry, reason }),
            Outcome::Keep(image) => images.push(image),
        }
        progress(index + 1, total);
    }
    Ok(Staged {
        dir: dir.to_path_buf(),
        source_name: source_name_of(zip_path),
        entries: total,
        images,
        skipped,
    })
}

enum Outcome {
    Ignore,
    Skip(String, SkipReason),
    Keep(StagedImage),
}

fn stage_entry(
    archive: &mut zip::ZipArchive<std::fs::File>,
    index: usize,
    dir: &Path,
    existing_hashes: &HashSet<String>,
    limits: &Limits,
    seen: &mut HashSet<String>,
    total_read: &mut u64,
) -> Result<Outcome, String> {
    // Classify by name and metadata first, through the raw reader (no decompression).
    let (shown, display, pre_skip) = {
        let entry = archive
            .by_index_raw(index)
            .map_err(|e| format!("The zip is damaged: {e}"))?;
        let raw = entry.name_raw().to_vec();
        if entry.is_dir() || raw.ends_with(b"/") || raw.ends_with(b"\\") {
            return Ok(Outcome::Ignore);
        }
        let lossy = String::from_utf8_lossy(&raw).into_owned();
        let skip = if is_unsafe_path(&raw) {
            Some(SkipReason::UnsafePath)
        } else if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            Some(SkipReason::Symlink)
        } else if has_macosx_component(&raw) {
            Some(SkipReason::MacMetadata)
        } else if has_hidden_component(&raw) {
            Some(SkipReason::Hidden)
        } else if entry.encrypted() {
            Some(SkipReason::Encrypted)
        } else if has_archive_extension(&lossy) {
            Some(SkipReason::NestedArchive)
        } else {
            None
        };
        let shown: String = lossy
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .take(120)
            .collect();
        (shown, sanitize_name(&lossy), skip)
    };
    if let Some(reason) = pre_skip {
        return Ok(Outcome::Skip(shown, reason));
    }

    // Read the bytes actually present, capped at the limit plus one. Header sizes are not
    // trusted.
    let mut buf: Vec<u8> = Vec::new();
    {
        let entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(_) => return Ok(Outcome::Skip(shown, SkipReason::BadImage)),
        };
        let read = entry.take(limits.max_file_bytes + 1).read_to_end(&mut buf);
        *total_read += buf.len() as u64;
        if *total_read > limits.max_total_bytes {
            return Err("The zip is too large once unpacked".to_string());
        }
        if read.is_err() && buf.len() as u64 <= limits.max_file_bytes {
            return Ok(Outcome::Skip(shown, SkipReason::BadImage));
        }
    }
    if buf.len() as u64 > limits.max_file_bytes {
        return Ok(Outcome::Skip(shown, SkipReason::TooLarge));
    }
    if is_archive_magic(&buf) {
        return Ok(Outcome::Skip(shown, SkipReason::NestedArchive));
    }
    let Some(format) = sniff(&buf) else {
        return Ok(Outcome::Skip(shown, SkipReason::NotImage));
    };
    let info = match inspect(&buf, format, limits) {
        Ok(info) => info,
        Err(reason) => return Ok(Outcome::Skip(shown, reason)),
    };
    let hash = hex(&Sha256::digest(&buf));
    if existing_hashes.contains(&hash) || !seen.insert(hash.clone()) {
        return Ok(Outcome::Skip(shown, SkipReason::Duplicate));
    }
    let file = format!("{}.{}", &hash[..16], format.extension());
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(&file))
        .map_err(|e| format!("Cannot write the staged image: {e}"))?;
    out.write_all(&buf)
        .map_err(|e| format!("Cannot write the staged image: {e}"))?;
    Ok(Outcome::Keep(StagedImage {
        hash,
        file,
        name: display,
        format,
        animated: info.animated,
        width: info.width,
        height: info.height,
        bytes: buf.len() as u64,
    }))
}

fn valid_pack_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Moves the staging folder to `emoji_dir/packs/<pack_id>` with one rename.
pub fn commit(staged: &Staged, emoji_dir: &Path, pack_id: &str) -> Result<PathBuf, String> {
    if !valid_pack_id(pack_id) {
        return Err("The pack id is not valid".to_string());
    }
    let is_staging = staged.dir.parent() == Some(emoji_dir)
        && staged
            .dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".staging-"));
    if !is_staging {
        return Err("The staging folder is not inside the emoji folder".to_string());
    }
    let packs = emoji_dir.join("packs");
    std::fs::create_dir_all(&packs).map_err(|e| format!("Cannot create the packs folder: {e}"))?;
    let target = packs.join(pack_id);
    if target.symlink_metadata().is_ok() {
        return Err("A pack with this id already exists".to_string());
    }
    std::fs::rename(&staged.dir, &target).map_err(|e| format!("Cannot save the pack: {e}"))?;
    Ok(target)
}

/// Removes the staging folder. Errors are ignored.
pub fn discard(staged: Staged) {
    let _ = std::fs::remove_dir_all(&staged.dir);
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
